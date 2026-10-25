//! A profile of Embassy tasks, sampled while the firmware runs: how often each
//! is running, how often it is in the run queue, and which `.await` it reached
//! last.
//!
//! Each sample reads every task's state flags and its future's state tag. Reads
//! of nearby tasks are merged, so this costs one probe round trip when the task
//! pools sit close together. The state tag changes only at an `.await`, so it
//! cannot tell a running task from one parked there. Each sample also reads the
//! program counter from the DWT's `PCSR`, which the core updates without
//! halting: a task runs when the PC is in its poll function, where the
//! compiler inlines the future's code.

use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};
use studio_dwarf::ElfInfo;
use studio_dwarf::tasks::{self, TaskProbe};

use crate::memory::{self, Memory};

/// Debug Exception and Monitor Control Register; bit 24 (TRCENA) enables the DWT
const DEMCR: u64 = 0xE000_EDFC;
const TRCENA: u64 = 1 << 24;
/// DWT Program Counter Sample Register
const PCSR: u64 = 0xE000_101C;
/// Functions listed outside tasks
const ELSEWHERE_LIMIT: usize = 5;

struct Profile {
    label: String,
    /// The poll function the task's code runs in, as an address range
    poll: Option<(u64, u64)>,
    running: u64,
    probe: TaskProbe,
    /// Where `probe`'s regions are in the merged reads: (read, offset, length)
    slices: Vec<(usize, usize, usize)>,
    spawned: u64,
    queued: u64,
    /// Samples per future state, by label, with where the state is in the source
    states: BTreeMap<String, (u64, Option<String>)>,
}

/// Sample every task for `duration`, then describe what each spent its time on.
pub fn profile(elf: &ElfInfo, memory: &mut dyn Memory, duration: Duration) -> Result<String> {
    let functions = Functions::new(elf);
    let mut profiles = Vec::new();
    for task in tasks::tasks(elf) {
        let probe = tasks::probe(elf, &task.root.node.node)
            .map_err(|e| anyhow::anyhow!("{}: {e}", task.root.node.label))?;
        // The task macro names the future's type after the task.
        let inner = match task.name.as_str() {
            "main" => "____embassy_main_task::".to_string(),
            name => format!("__{name}_task::"),
        };
        profiles.push(Profile {
            label: task.root.node.label.clone(),
            poll: functions.find(|name| {
                name.contains("TaskStorage<") && name.contains(&inner) && name.ends_with(">::poll")
            }),
            running: 0,
            probe,
            slices: Vec::new(),
            spawned: 0,
            queued: 0,
            states: BTreeMap::new(),
        });
    }
    if profiles.is_empty() {
        bail!("the ELF has no Embassy tasks");
    }
    let reads = plan(&mut profiles, memory.ram());
    let sample_pc = enable_pc_sampling(memory);
    // Samples by function, for those outside every task
    let mut elsewhere: BTreeMap<String, u64> = BTreeMap::new();

    let start = Instant::now();
    let mut samples = 0u64;
    while samples == 0 || start.elapsed() < duration {
        let bytes = reads
            .iter()
            .map(|&(address, len)| memory.transfer(address, len))
            .collect::<Result<Vec<_>>>()?;
        if sample_pc {
            // Bit 0 is the Thumb bit; all ones means no sample, such as while halted.
            let pc = memory.word(PCSR, 4)? & !1;
            match profiles.iter_mut().find(|p| {
                p.poll
                    .is_some_and(|(start, end)| (start..end).contains(&pc))
            }) {
                Some(profile) => profile.running += 1,
                None => {
                    let name = match pc {
                        0xffff_fffe => "no sample".to_string(),
                        pc => functions
                            .name_at(pc)
                            .map_or(format!("{pc:#010x}"), short_name),
                    };
                    *elsewhere.entry(name).or_default() += 1;
                }
            }
        }
        for profile in &mut profiles {
            let regions: Vec<Vec<u8>> = profile
                .slices
                .iter()
                .map(|&(read, offset, len)| bytes[read][offset..offset + len].to_vec())
                .collect();
            let Some(state) = profile.probe.decode(&regions) else {
                continue;
            };
            if !state.spawned {
                continue;
            }
            profile.spawned += 1;
            profile.queued += u64::from(state.queued);
            if let Some(at) = state.at {
                let location = at
                    .location
                    .map(|l| format!("{}:{}", shorten(&l.file), l.line));
                profile.states.entry(at.label).or_insert((0, location)).0 += 1;
            }
        }
        samples += 1;
    }
    let elapsed = start.elapsed();

    let mut out = String::new();
    for profile in &profiles {
        if profile.spawned == 0 {
            writeln!(out, "{}: not spawned", profile.label)?;
            continue;
        }
        let percent = |count: u64| 100.0 * count as f64 / profile.spawned as f64;
        write!(out, "{}: ", profile.label)?;
        if sample_pc {
            write!(out, "running {}, ", share(profile.running, samples))?;
        }
        write!(out, "run-queued {:.0}%", percent(profile.queued))?;
        if profile.spawned < samples {
            write!(
                out,
                ", spawned in {:.0}% of samples",
                100.0 * profile.spawned as f64 / samples as f64
            )?;
        }
        writeln!(out)?;
        let mut states: Vec<_> = profile.states.iter().collect();
        states.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (label, (count, location)) in states {
            let at = location
                .as_ref()
                .map_or(String::new(), |l| format!(" at {l}"));
            writeln!(out, "  {:>5.1}%  {label}{at}", percent(*count))?;
        }
    }
    if sample_pc {
        let mut functions: Vec<_> = elsewhere.into_iter().collect();
        functions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut parts: Vec<String> = functions
            .iter()
            .take(ELSEWHERE_LIMIT)
            .map(|(name, count)| format!("{name} {}", share(*count, samples)))
            .collect();
        let rest: u64 = functions.iter().skip(ELSEWHERE_LIMIT).map(|(_, c)| c).sum();
        if rest > 0 {
            parts.push(format!("other {}", share(rest, samples)));
        }
        if !parts.is_empty() {
            writeln!(out, "outside tasks: {}", parts.join(", "))?;
        }
    } else {
        writeln!(
            out,
            "the core has no PC sampling, so running time is not shown"
        )?;
    }
    writeln!(
        out,
        "{samples} sample{} in {:.2} s, {:.0} per second",
        if samples == 1 { "" } else { "s" },
        elapsed.as_secs_f64(),
        samples as f64 / elapsed.as_secs_f64().max(f64::EPSILON)
    )?;
    Ok(out)
}

/// `count` of `total` as a whole percentage, with `<1%` for a few samples rather
/// than a misleading `0%`.
fn share(count: u64, total: u64) -> String {
    let percent = 100.0 * count as f64 / total.max(1) as f64;
    if count > 0 && percent < 0.5 {
        "<1%".into()
    } else {
        format!("{percent:.0}%")
    }
}

/// Turn on the DWT if it is off, and check that `PCSR` samples. Setting TRCENA
/// changes nothing the firmware sees; debuggers set it for watchpoints too.
fn enable_pc_sampling(memory: &mut dyn Memory) -> bool {
    let Ok(demcr) = memory.word(DEMCR, 4) else {
        return false;
    };
    if demcr & TRCENA == 0
        && memory
            .write(DEMCR, &((demcr | TRCENA) as u32).to_le_bytes())
            .is_err()
    {
        return false;
    }
    // A core without PCSR reads it as zero.
    (0..3).any(|_| memory.word(PCSR, 4).is_ok_and(|pc| pc != 0))
}

/// Functions by address, to name the one a sampled PC is in
pub struct Functions {
    /// (start, end, name), sorted by start
    ranges: Vec<(u64, u64, String)>,
}

impl Functions {
    pub fn new(elf: &ElfInfo) -> Self {
        let mut ranges: Vec<_> = elf
            .get_functions()
            .into_iter()
            .filter(|f| f.size > 0)
            .map(|f| {
                let start = f.address & !1;
                (start, start + f.size, f.demangled_name.clone())
            })
            .collect();
        ranges.sort();
        Self { ranges }
    }

    fn find(&self, matches: impl Fn(&str) -> bool) -> Option<(u64, u64)> {
        self.ranges
            .iter()
            .find(|(_, _, name)| matches(name))
            .map(|&(start, end, _)| (start, end))
    }

    pub fn name_at(&self, pc: u64) -> Option<&str> {
        let i = self.ranges.partition_point(|&(start, _, _)| start <= pc);
        let (_, end, name) = self.ranges.get(i.checked_sub(1)?)?;
        (pc < *end).then_some(name.as_str())
    }
}

/// `<embassy_executor::platform::thread::Executor>::run::<…>` as `Executor::run`:
/// without generic arguments, and only the last type or module and the function.
pub fn short_name(name: &str) -> String {
    let mut plain = String::new();
    let mut depth = 0usize;
    let mut chars = name.chars().peekable();
    // A leading `<Type>` is the type a method is on; keep it, without its arguments.
    let wrapped = name.starts_with('<');
    if wrapped {
        chars.next();
    }
    for c in chars {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            '>' if wrapped => {}
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let plain = plain.trim_end_matches("::");
    let parts: Vec<&str> = plain.split("::").filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(2)..].join("::")
}

/// Merge every task's regions into as few reads as possible, and record where
/// each region lands in them.
fn plan(profiles: &mut [Profile], ram: &[std::ops::Range<u64>]) -> Vec<(u64, usize)> {
    let reads = memory::merge(profiles.iter().flat_map(|p| p.probe.regions()), ram);
    for profile in profiles {
        profile.slices = profile
            .probe
            .regions()
            .into_iter()
            .map(|(address, len)| {
                let read = reads
                    .iter()
                    .position(|&(start, size)| {
                        address >= start && address + len as u64 <= start + size as u64
                    })
                    .expect("every region is inside a read");
                (read, (address - reads[read].0) as usize, len)
            })
            .collect();
    }
    reads
}

/// A source path relative to the current directory when it is inside it.
fn shorten(file: &str) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|dir| {
            Path::new(file)
                .strip_prefix(dir)
                .ok()
                .map(Path::to_path_buf)
        })
        .map_or_else(|| file.to_string(), |p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Fake;
    use studio_dwarf::ElfParser;

    #[test]
    fn profiles_the_lab_tasks() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/select_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        // The ELF's own image: .bss is zero, so no task is spawned.
        let mut memory = Fake::from_elf(path);
        let text = profile(&elf, &mut memory, Duration::ZERO).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.contains(&"waves: not spawned"), "{text}");
        assert!(lines.contains(&"control: not spawned"), "{text}");
        assert!(lines.last().unwrap().starts_with("1 sample in"), "{text}");

        // Spawn `control` and queue it, parked at its first `.await`. An async
        // fn's states are Unresumed, Returned and Panicked, then Suspend0 at 3.
        let control = tasks::tasks(&elf)
            .into_iter()
            .find(|t| t.root.node.label == "control")
            .unwrap();
        let regions = tasks::probe(&elf, &control.root.node.node)
            .unwrap()
            .regions();
        let (flags, flags_len) = regions[0];
        memory.write(flags, &vec![0xff; flags_len]).unwrap();
        let (tag, tag_len) = regions[1];
        let mut value = vec![0; tag_len];
        value[0] = 3;
        memory.write(tag, &value).unwrap();
        let text = profile(&elf, &mut memory, Duration::ZERO).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let at = lines.iter().position(|l| *l == "control: run-queued 100%");
        let state = at.map(|i| lines[i + 1]).unwrap_or_default();
        assert!(
            state.starts_with("  100.0%  Suspend0 at ") && state.contains("main.rs:"),
            "{text}"
        );
        // Fake memory has no DWT.
        assert!(text.contains("the core has no PC sampling"), "{text}");
    }

    #[test]
    fn shortens_function_names() {
        for (name, short) in [
            (
                "<embassy_executor::platform::thread::Executor>::run::<select_lab::__cortex_m_rt_main::{closure#0}>",
                "Executor::run",
            ),
            ("<tuning_studio_api::Param<u32>>::get", "Param::get"),
            ("libm::math::k_sinf::k_sinf", "k_sinf::k_sinf"),
            ("SysTick", "SysTick"),
        ] {
            assert_eq!(short_name(name), short, "{name}");
        }
    }
}
