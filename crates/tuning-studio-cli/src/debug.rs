//! Run control for core 0: halt, resume, single step and hardware breakpoints.
//! probe-rs clears every hardware breakpoint when a session attaches or detaches,
//! so breakpoints last only as long as one `session`.

use anyhow::{Context, Result, bail};
use probe_rs::{BreakpointCause, CoreStatus, HaltReason, Session};
use std::time::Duration;
use studio_dwarf::{ElfInfo, SymbolInfo};

const HALT_TIMEOUT: Duration = Duration::from_millis(100);

pub struct Breakpoint {
    pub id: u32,
    pub address: u64,
    pub label: String,
}

#[derive(Default)]
pub struct Breakpoints {
    list: Vec<Breakpoint>,
    next_id: u32,
}

impl Breakpoints {
    pub fn iter(&self) -> impl Iterator<Item = &Breakpoint> {
        self.list.iter()
    }

    /// Set a hardware breakpoint on `text`, a function name or an address.
    pub fn add(
        &mut self,
        elf: Option<&ElfInfo>,
        session: &mut Session,
        text: &str,
    ) -> Result<&Breakpoint> {
        let address = match parse_address(text) {
            Some(address) => address,
            None => {
                let elf = elf.context("function names need --elf")?;
                find_function(elf, text)?.address & !1
            }
        };
        if self.list.iter().any(|b| b.address == address) {
            bail!("a breakpoint is already set at {address:#010x}");
        }
        session
            .core(0)?
            .set_hw_breakpoint(address)
            .with_context(|| format!("could not set a breakpoint at {address:#010x}"))?;
        self.next_id += 1;
        self.list.push(Breakpoint {
            id: self.next_id,
            address,
            label: describe_address(elf, address),
        });
        Ok(self.list.last().unwrap())
    }

    /// Clear the breakpoints in `ids`, or all of them when `ids` is empty.
    pub fn delete(&mut self, session: &mut Session, ids: &[u32]) -> Result<Vec<Breakpoint>> {
        if let Some(id) = ids
            .iter()
            .find(|id| !self.list.iter().any(|b| b.id == **id))
        {
            bail!("no breakpoint {id}");
        }
        let mut core = session.core(0)?;
        let (gone, kept) = std::mem::take(&mut self.list)
            .into_iter()
            .partition(|b| ids.is_empty() || ids.contains(&b.id));
        self.list = kept;
        for breakpoint in &gone {
            core.clear_hw_breakpoint(breakpoint.address)?;
        }
        Ok(gone)
    }
}

pub fn halt(session: &mut Session) -> Result<()> {
    let mut core = session.core(0)?;
    if !core.core_halted()? {
        core.halt(HALT_TIMEOUT)?;
    }
    Ok(())
}

pub fn resume(session: &mut Session) -> Result<()> {
    let mut core = session.core(0)?;
    if core.core_halted()? {
        core.run()?;
    }
    Ok(())
}

pub fn step(session: &mut Session) -> Result<()> {
    let mut core = session.core(0)?;
    if !core.core_halted()? {
        bail!("the core is running; halt it first");
    }
    core.step()?;
    Ok(())
}

/// The core's status. A core waiting in `wfi` counts as running: firmware that
/// sleeps between interrupts would otherwise switch status on nearly every poll.
pub fn status(session: &mut Session) -> Result<CoreStatus> {
    Ok(match session.core(0)?.status()? {
        CoreStatus::Sleeping => CoreStatus::Running,
        status => status,
    })
}

/// One line for a core status, such as `halted at 0x08000512 (main + 0x12), breakpoint 1`.
pub fn describe_status(
    elf: Option<&ElfInfo>,
    session: &mut Session,
    breakpoints: &Breakpoints,
    status: CoreStatus,
) -> Result<String> {
    let reason = match status {
        CoreStatus::Running | CoreStatus::Sleeping => return Ok("running".into()),
        CoreStatus::LockedUp => return Ok("locked up".into()),
        CoreStatus::Unknown => return Ok("status unknown".into()),
        CoreStatus::Halted(reason) => reason,
    };
    let mut core = session.core(0)?;
    let pc: u64 = core.read_core_reg(core.program_counter())?;
    let reason = match reason {
        HaltReason::Breakpoint(BreakpointCause::Hardware | BreakpointCause::Unknown) => breakpoints
            .iter()
            .find(|b| b.address == pc)
            .map_or("breakpoint".to_string(), |b| format!("breakpoint {}", b.id)),
        HaltReason::Breakpoint(BreakpointCause::Software) => "software breakpoint".into(),
        HaltReason::Breakpoint(BreakpointCause::Semihosting(_)) => "semihosting".into(),
        HaltReason::Request => "on request".into(),
        HaltReason::Step => "after a step".into(),
        HaltReason::Exception => "on an exception".into(),
        HaltReason::Watchpoint => "watchpoint".into(),
        HaltReason::External => "by an external request".into(),
        HaltReason::Multiple => "for several reasons".into(),
        HaltReason::Unknown => "for an unknown reason".into(),
    };
    Ok(format!("halted at {}, {reason}", describe_address(elf, pc)))
}

/// `0x08000512 (main + 0x12)`, or the bare address outside every function.
pub fn describe_address(elf: Option<&ElfInfo>, address: u64) -> String {
    let function = elf.and_then(|elf| {
        elf.get_functions()
            .into_iter()
            .map(|f| (f, f.address & !1))
            .find(|&(f, start)| start <= address && address < start + f.size.max(1))
    });
    match function {
        Some((f, start)) if address == start => format!("{address:#010x} ({})", f.demangled_name),
        Some((f, start)) => format!(
            "{address:#010x} ({} + {:#x})",
            f.demangled_name,
            address - start
        ),
        None => format!("{address:#010x}"),
    }
}

/// A function by its full name, or by a `::name` suffix that only one function has.
fn find_function<'e>(elf: &'e ElfInfo, name: &str) -> Result<&'e SymbolInfo> {
    let functions = elf.get_functions();
    if let Some(f) = functions
        .iter()
        .find(|f| f.demangled_name == name || f.mangled_name == name)
    {
        return Ok(f);
    }
    let suffix = format!("::{name}");
    let mut matches: Vec<_> = functions
        .into_iter()
        .filter(|f| f.demangled_name.ends_with(&suffix))
        .collect();
    matches.sort_by(|a, b| a.demangled_name.cmp(&b.demangled_name));
    matches.dedup_by(|a, b| a.address == b.address);
    match matches.as_slice() {
        [] => bail!("no function named `{name}` in the ELF"),
        [only] => Ok(only),
        _ => {
            let names: Vec<_> = matches.iter().map(|f| f.demangled_name.as_str()).collect();
            bail!(
                "`{name}` matches {}; use more of the path",
                names.join(", ")
            )
        }
    }
}

fn parse_address(text: &str) -> Option<u64> {
    let text = text.replace('_', "");
    // The target is 32-bit, so a wider address is not an address.
    let address = match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse::<u32>().ok(),
    };
    address.map(u64::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_dwarf::ElfParser;

    #[test]
    fn breakpoint_addresses_are_32_bit() {
        assert_eq!(parse_address("0xFFFF_FFFF"), Some(0xFFFF_FFFF));
        assert_eq!(parse_address("0x1_0000_0000"), None);
        assert_eq!(parse_address("4294967296"), None);
    }

    #[test]
    fn finds_lab_functions() {
        let elf = ElfParser::parse(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/type_lab.elf"
        ))
        .unwrap();
        let systick = find_function(&elf, "SysTick").unwrap();
        let start = systick.address & !1;
        assert_eq!(
            describe_address(Some(&elf), start),
            format!("{start:#010x} (SysTick)")
        );
        assert_eq!(
            describe_address(Some(&elf), start + 4),
            format!("{:#010x} (SysTick + 0x4)", start + 4)
        );
        assert!(find_function(&elf, "no_such_function").is_err());
    }
}
