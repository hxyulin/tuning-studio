mod debug;
mod flash;
mod gdb;
mod logs;
mod memory;
mod path;
mod session;
mod target;
mod value;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfInfo, ElfParser, Step};

use memory::Memory;
use path::Location;

#[derive(Parser)]
#[command(
    name = "tuning-studio",
    version,
    about = "Inspect and tune a running target over SWD"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read variables or addresses once
    Read {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol paths, such as `CONTROL.yaw.target_rpm` or `buffer[3]`, or addresses
        /// with an optional type and path, such as `0x24000070`, `0x24000070:f32` or
        /// `0x20000038:Axis.gains.2`. Types other than the primitives need `--elf`.
        #[arg(required = true)]
        locations: Vec<String>,
    },
    /// Read variables or addresses repeatedly until Ctrl-C
    Watch {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol paths or addresses, as for `read`
        #[arg(required = true)]
        locations: Vec<String>,
        /// Time between reads in milliseconds
        #[arg(long, default_value_t = 100)]
        interval: u64,
        /// Stop after this many reads
        #[arg(long)]
        count: Option<u64>,
    },
    /// Write one value, then read it back
    Set {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol path or address, as for `read`
        location: String,
        #[arg(allow_negative_numbers = true)]
        value: String,
    },
    /// Attach once and run commands from stdin, one per line: read, watch, set,
    /// halt, resume, step, break and more. `help` lists them.
    Session {
        #[command(flatten)]
        target: TargetArgs,
        /// Also serve GDB on this address; `--gdb` alone uses 127.0.0.1:1337
        #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:1337")]
        gdb: Option<SocketAddr>,
    },
    /// Flash and reset the firmware, then print its RTT logs and take session
    /// commands on stdin until Ctrl-C. Works as a Cargo runner.
    Run {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Firmware ELF
        elf: PathBuf,
        /// Read the flash back after programming
        #[arg(long)]
        verify: bool,
        /// Also serve GDB on this address; `--gdb` alone uses 127.0.0.1:1337
        #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:1337")]
        gdb: Option<SocketAddr>,
    },
    /// Print the RTT logs of running firmware and take session commands on stdin,
    /// without flashing or resetting
    Attach {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Firmware ELF
        elf: PathBuf,
        /// Also serve GDB on this address; `--gdb` alone uses 127.0.0.1:1337
        #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:1337")]
        gdb: Option<SocketAddr>,
    },
    /// Program an ELF into flash, then reset
    Flash {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Firmware ELF
        elf: PathBuf,
        /// Read the flash back after programming
        #[arg(long)]
        verify: bool,
        /// Erase the whole chip rather than only the sectors the ELF uses
        #[arg(long)]
        chip_erase: bool,
        /// Halt at the reset vector instead of running
        #[arg(long)]
        halt: bool,
    },
    /// Erase all flash
    Erase {
        #[command(flatten)]
        probe: ProbeArgs,
    },
    /// Reset the target
    Reset {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Halt at the reset vector instead of running
        #[arg(long)]
        halt: bool,
    },
    /// List the connected debug probes
    List,
}

#[derive(Args)]
struct TargetArgs {
    /// Firmware ELF with DWARF debug information, needed for symbol names
    #[arg(long)]
    elf: Option<PathBuf>,
    #[command(flatten)]
    probe: ProbeArgs,
}

#[derive(Args)]
struct ProbeArgs {
    /// probe-rs chip name, such as `STM32H723VGTx`
    #[arg(long)]
    chip: String,
    /// Probe as `VID:PID` or `VID:PID:SERIAL`, needed when several are connected
    #[arg(long)]
    probe: Option<String>,
    /// SWD clock in kHz
    #[arg(long, default_value_t = 4000)]
    speed: u32,
}

impl ProbeArgs {
    fn attach(&self) -> Result<target::Target> {
        target::Target::attach(&self.chip, self.probe.as_deref(), self.speed, false)
    }
}

impl TargetArgs {
    fn attach(&self) -> Result<target::Target> {
        self.probe.attach()
    }

    fn elf(&self) -> Result<Option<ElfInfo>> {
        self.elf.as_deref().map(parse_elf).transpose()
    }
}

fn parse_elf(path: &std::path::Path) -> Result<ElfInfo> {
    ElfParser::parse(path).with_context(|| format!("could not parse {}", path.display()))
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Read { target, locations } => {
            let elf = target.elf()?;
            let probe = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            for location in &locations {
                let location = path::locate(elf.as_ref(), &mut memory, location)?;
                print!("{}", read(elf.as_ref(), &mut memory, &location)?);
            }
        }
        Command::Watch {
            target,
            locations,
            interval,
            count,
        } => {
            let elf = target.elf()?;
            let probe = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            let locations = locations
                .iter()
                .map(|location| path::locate(elf.as_ref(), &mut memory, location))
                .collect::<Result<Vec<_>>>()?;
            let interval = Duration::from_millis(interval.max(1));
            watch(elf.as_ref(), &mut memory, &locations, interval, count)?;
        }
        Command::Set {
            target,
            location,
            value,
        } => {
            let elf = target.elf()?;
            let probe = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            let location = path::locate(elf.as_ref(), &mut memory, &location)?;
            set(elf.as_ref(), &mut memory, &location, &value)?;
            print!("{}", read(elf.as_ref(), &mut memory, &location)?);
        }
        Command::Session { target, gdb } => {
            let elf = target.elf()?;
            let probe = target.attach()?;
            let options = session::Options {
                elf: target.elf,
                gdb,
                logs: false,
            };
            session::run(&probe, elf, options)?;
        }
        Command::Run {
            probe,
            elf,
            verify,
            gdb,
        } => {
            let info = parse_elf(&elf)?;
            let target = probe.attach()?;
            {
                let mut session = target.session.lock();
                flash::flash(&mut session, &elf, verify, false)?;
                if let Some(mut logs) = logs::Logs::new(&elf)? {
                    logs.restart(&mut session)?;
                }
                flash::reset(&mut session, false)?;
            }
            let options = session::Options {
                elf: Some(elf),
                gdb,
                logs: true,
            };
            session::run(&target, Some(info), options)?;
        }
        Command::Attach { probe, elf, gdb } => {
            let info = parse_elf(&elf)?;
            let target = probe.attach()?;
            let options = session::Options {
                elf: Some(elf),
                gdb,
                logs: true,
            };
            session::run(&target, Some(info), options)?;
        }
        Command::Flash {
            probe,
            elf,
            verify,
            chip_erase,
            halt,
        } => {
            let target = probe.attach()?;
            let mut session = target.session.lock();
            flash::flash(&mut session, &elf, verify, chip_erase)?;
            flash::reset(&mut session, halt)?;
        }
        Command::Erase { probe } => {
            let target =
                target::Target::attach(&probe.chip, probe.probe.as_deref(), probe.speed, true)?;
            flash::erase(&mut target.session.lock())?;
        }
        Command::Reset { probe, halt } => {
            let target = probe.attach()?;
            flash::reset(&mut target.session.lock(), halt)?;
        }
        Command::List => target::list(),
    }
    Ok(())
}

/// Print every location each `interval`, until Ctrl-C or `count` reads. A terminal
/// shows the latest read in place; other output gets every read after a time line.
/// The probe detaches normally on Ctrl-C.
fn watch(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    locations: &[Location],
    interval: Duration,
    count: Option<u64>,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let handler = Arc::clone(&stop);
    ctrlc::set_handler(move || handler.store(true, Ordering::Relaxed))?;
    session::block_sigint();
    let terminal = std::io::stdout().is_terminal();
    let start = Instant::now();
    let mut next = start;
    let mut reads = 0;
    while !stop.load(Ordering::Relaxed) && count.is_none_or(|count| reads < count) {
        let elapsed = start.elapsed().as_secs_f64();
        let mut text = String::new();
        for location in locations {
            let location = refresh(elf, memory, location)?;
            text += &read(elf, memory, &location)?;
        }
        let mut out = std::io::stdout().lock();
        if terminal {
            // Home the cursor and clear the screen, as watch(1) does.
            write!(out, "\x1b[H\x1b[2J")?;
            writeln!(
                out,
                "{elapsed:.1} s, every {} ms; Ctrl-C stops",
                interval.as_millis()
            )?;
        } else {
            writeln!(out, "--- {elapsed:.3} s")?;
        }
        out.write_all(text.as_bytes())?;
        out.flush()?;
        drop(out);
        reads += 1;
        // Keep to the schedule; after a slow read, skip the reads it overran.
        next += interval;
        let now = Instant::now();
        if next < now {
            next += interval * ((now - next).as_nanos() / interval.as_nanos()) as u32 + interval;
        }
        while !stop.load(Ordering::Relaxed) && Instant::now() < next {
            std::thread::sleep((next - Instant::now()).min(Duration::from_millis(20)));
        }
    }
    Ok(())
}

/// Resolve a location again, so paths through pointers follow their current targets.
fn refresh(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    location: &Location,
) -> Result<Location> {
    Ok(match (location, elf) {
        (Location::Node(node), Some(elf)) => {
            Location::Node(Box::new(path::describe(elf, memory, &node.node)?))
        }
        (&Location::Raw { address, scalar }, _) => Location::Raw { address, scalar },
        (Location::Node(_), None) => unreachable!("symbol paths need an ELF"),
    })
}

fn read(elf: Option<&ElfInfo>, memory: &mut dyn Memory, location: &Location) -> Result<String> {
    match (location, elf) {
        (Location::Node(node), Some(elf)) => value::render(elf, memory, node),
        (&Location::Raw { address, scalar }, _) => {
            let bytes = memory.read(address, value::size_of(scalar))?;
            Ok(format!(
                "{address:#010x}: {scalar} = {}\n",
                value::decode(scalar, &bytes)
            ))
        }
        (Location::Node(_), None) => unreachable!("symbol paths need an ELF"),
    }
}

fn set(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    location: &Location,
    text: &str,
) -> Result<()> {
    let (address, bytes) = match location {
        &Location::Raw { address, scalar } => (address, value::encode(scalar, text)?),
        Location::Node(node) => {
            let elf = elf.expect("symbol paths need an ELF");
            let path = &node.path;
            if !node.readable {
                bail!(
                    "{path} is not readable: {}",
                    node.status.as_deref().unwrap_or("unknown")
                );
            }
            if node.bit_size.is_some() {
                bail!("{path} is a bitfield; writing bitfields is not supported yet");
            }
            let (NodeKind::Scalar | NodeKind::Enum, Some(scalar)) =
                (node.kind, value::scalar_of(node))
            else {
                bail!(
                    "{path} is a {}; set one of its fields instead",
                    node.type_name
                );
            };
            // Behind a pointer the target section is unknown, so only direct fields are checked.
            let direct = !node
                .node
                .steps
                .iter()
                .any(|s| matches!(s, Step::Deref | Step::SliceIndex(_)));
            if direct
                && elf
                    .find_symbol(&node.node.symbol)
                    .is_some_and(|s| !s.writable)
            {
                bail!("{path} is in a read-only section");
            }
            let bytes = if node.kind == NodeKind::Enum {
                let variants = tree::enum_variants(elf, &node.node, &mut |address, len| {
                    memory.word(address, len).map_err(|e| format!("{e:#}"))
                })
                .map_err(|e| anyhow!("{e}"))?
                .unwrap_or_default();
                value::encode_enum(&variants, value::size_of(scalar), text)?
            } else if node.type_name == "char" {
                value::encode_char(text)?
            } else {
                value::encode(scalar, text)?
            };
            (node.address, bytes)
        }
    };
    memory.write(address, &bytes)?;
    if memory.read(address, bytes.len())? != bytes {
        eprintln!(
            "warning: the value read back differs from the value written. The firmware may \
             have changed it since, or this memory ignores writes."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::Fake;

    #[test]
    fn sets_scalars_only() {
        let elf = ElfParser::parse(format!(
            "{}/../studio-dwarf/tests/fixtures/test_struct.elf",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let symbol = elf.find_symbol("device_config").unwrap();
        let mut memory = Fake::new(symbol.address, vec![0; symbol.size as usize]);
        let location = path::locate(Some(&elf), &mut memory, "device_config.sensor.value").unwrap();
        set(Some(&elf), &mut memory, &location, "3.25").unwrap();
        assert_eq!(
            read(Some(&elf), &mut memory, &location).unwrap(),
            "device_config.sensor.value: float = 3.25\n"
        );
        let location = path::locate(Some(&elf), &mut memory, "device_config.enabled").unwrap();
        assert!(set(Some(&elf), &mut memory, &location, "256").is_err());
        let location = path::locate(Some(&elf), &mut memory, "device_config.sensor").unwrap();
        assert!(set(Some(&elf), &mut memory, &location, "1").is_err());
    }

    #[test]
    fn sets_lab_enums_chars_and_atomics() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        let mut memory = Fake::from_elf(path);
        for (location, value, expected) in [
            ("enums::DIRECTION", "Forward", "Direction = Forward (1)"),
            ("enums::DIRECTION", "-1", "Direction = Reverse (-1)"),
            ("scalars::CHAR", "😀", "char = '😀' (U+1F600)"),
            ("atomics::BOOL", "false", "Atomic<bool> = false"),
            ("atomics::I16", "-2", "Atomic<i16> = -2"),
            ("ROBOT.id.1", "-100", "i16 = -100"),
        ] {
            let location = path::locate(Some(&elf), &mut memory, location).unwrap();
            set(Some(&elf), &mut memory, &location, value).unwrap();
            let text = read(Some(&elf), &mut memory, &location).unwrap();
            assert!(text.ends_with(&format!(": {expected}\n")), "{text}");
        }
        for (location, value) in [
            ("enums::DIRECTION", "5"),
            ("enums::MODE", "Sideways"),
            ("atomics::BOOL", "2"),
            ("atomics::PTR", "0"),
            ("TABLE[0]", "9"),
        ] {
            let location = path::locate(Some(&elf), &mut memory, location).unwrap();
            assert!(set(Some(&elf), &mut memory, &location, value).is_err());
        }
    }
}
