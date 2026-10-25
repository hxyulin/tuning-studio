mod api;
mod daemon;
mod debug;
mod flash;
mod gdb;
mod logs;
mod memory;
mod path;
mod session;
mod target;
mod tasks;
mod value;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfInfo, ElfParser, Step};

use memory::Memory;
use path::Location;

/// Inspect and tune a running target over SWD.
///
/// `run` and `attach` keep the probe and serve the other commands, so `read`,
/// `set`, `watch`, `halt`, `break` and the rest go to the running session and
/// need no `--chip` or `--elf`. Without a session, most commands attach for
/// their own run instead.
#[derive(Parser)]
#[command(name = "tuning-studio", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Flash and reset the firmware, then print its RTT logs and serve commands
    /// until Ctrl-C. Works as a Cargo runner.
    Run {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Firmware ELF
        elf: PathBuf,
        /// Read the flash back after programming
        #[arg(long)]
        verify: bool,
        #[command(flatten)]
        serve: ServeArgs,
    },
    /// Print the RTT logs of running firmware and serve commands until Ctrl-C,
    /// without flashing or resetting
    Attach {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Firmware ELF, for symbols and logs
        elf: Option<PathBuf>,
        #[command(flatten)]
        serve: ServeArgs,
    },
    /// Read variables or addresses once
    Read {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol paths, such as `CONTROL.yaw.target_rpm` or `buffer[3]`, or addresses
        /// with an optional type and path, such as `0x24000070`, `0x24000070:f32` or
        /// `0x20000038:Axis.gains.2`. Types other than the primitives need an ELF.
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
        /// Time between reads in milliseconds; 0 reads as fast as the probe can.
        /// A session reads at most every 1 ms.
        #[arg(long, default_value_t = 100)]
        interval: u64,
        /// Stop after this many reads
        #[arg(long)]
        count: Option<u64>,
    },
    /// Sample every Embassy task while the firmware runs, then print how often
    /// each was run-queued and which `.await` it was parked on
    Tasks {
        #[command(flatten)]
        target: TargetArgs,
        /// How long to sample, in milliseconds
        #[arg(long, default_value_t = 1000)]
        duration: u64,
    },
    /// Print every `Param` and `watch!` value the firmware declares with
    /// tuning-studio-api
    Values {
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Write one value, then read it back. A `Param` changes through the
    /// firmware, which applies its range and step.
    Set {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol path or address, as for `read`
        location: String,
        #[arg(allow_negative_numbers = true)]
        value: String,
    },
    /// Halt the core
    Halt {
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Resume the core
    Resume {
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Execute one instruction on a halted core
    Step {
        #[command(flatten)]
        target: TargetArgs,
    },
    /// Set a hardware breakpoint on a function or an address. Needs a running
    /// session, since probe-rs clears breakpoints on detach.
    Break {
        /// Probe of the session, needed when several are running
        #[arg(long)]
        probe: Option<String>,
        /// Function name, such as `SysTick` or `control::tick`, or an address
        target: String,
    },
    /// Clear the breakpoints with these numbers, or all of them
    Delete {
        /// Probe of the session, needed when several are running
        #[arg(long)]
        probe: Option<String>,
        ids: Vec<u32>,
    },
    /// Print the core status, and a session's breakpoints and watches
    Status {
        #[command(flatten)]
        target: TargetArgs,
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
    /// Reset the target
    Reset {
        #[command(flatten)]
        probe: ProbeArgs,
        /// Halt at the reset vector instead of running
        #[arg(long)]
        halt: bool,
    },
    /// Erase all flash
    Erase {
        #[command(flatten)]
        probe: ProbeArgs,
    },
    /// List the connected debug probes
    List,
}

#[derive(Args)]
struct ServeArgs {
    /// Also take commands from stdin, one per line; `help` lists them
    #[arg(short, long)]
    interactive: bool,
    /// Also serve GDB on this address; `--gdb` alone uses 127.0.0.1:1337
    #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:1337")]
    gdb: Option<SocketAddr>,
}

#[derive(Args)]
struct TargetArgs {
    /// Firmware ELF with DWARF debug information, needed for symbol names without a
    /// session
    #[arg(long)]
    elf: Option<PathBuf>,
    #[command(flatten)]
    probe: ProbeArgs,
}

#[derive(Args)]
struct ProbeArgs {
    /// probe-rs chip name, such as `STM32H723VGTx`; not needed with a session
    #[arg(long)]
    chip: Option<String>,
    /// Probe as `VID:PID` or `VID:PID:SERIAL`, needed when several are connected
    #[arg(long)]
    probe: Option<String>,
    /// SWD clock in kHz
    #[arg(long, default_value_t = 4000)]
    speed: u32,
}

impl ProbeArgs {
    fn attach(&self, erase_all: bool) -> Result<target::Target> {
        let chip = self
            .chip
            .as_deref()
            .ok_or_else(|| anyhow!("--chip is needed when no `run` or `attach` is running"))?;
        target::Target::attach(chip, self.probe.as_deref(), self.speed, erase_all)
    }

    /// The running session for this probe
    fn session(&self) -> Result<Option<std::os::unix::net::UnixStream>> {
        daemon::find(self.probe.as_deref())
    }

    /// Fail when a session holds the probe, since it would make attaching fail
    /// with a less clear error.
    fn no_session(&self, command: &str) -> Result<()> {
        if self.session()?.is_some() {
            bail!("`{command}` needs the probe; stop the running `run` or `attach` first");
        }
        Ok(())
    }
}

impl TargetArgs {
    fn elf(&self) -> Result<Option<ElfInfo>> {
        self.elf.as_deref().map(parse_elf).transpose()
    }

    /// Attach for one command, with the ELF if one was given.
    fn attach(&self) -> Result<(target::Target, Option<ElfInfo>)> {
        let elf = self.elf()?;
        Ok((self.probe.attach(false)?, elf))
    }
}

fn parse_elf(path: &Path) -> Result<ElfInfo> {
    ElfParser::parse(path).with_context(|| format!("could not parse {}", path.display()))
}

/// The session grammar's words for a command
fn words(command: &str, rest: impl IntoIterator<Item = String>) -> Vec<String> {
    std::iter::once(command.to_string()).chain(rest).collect()
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run {
            probe,
            elf,
            verify,
            serve,
        } => {
            probe.no_session("run")?;
            let info = parse_elf(&elf)?;
            let target = probe.attach(false)?;
            {
                let mut session = target.session.lock();
                flash::flash(&mut session, &elf, verify, false)?;
                if let Some(mut logs) = logs::Logs::new(&elf)? {
                    logs.restart(&mut session)?;
                }
                flash::reset(&mut session, false)?;
            }
            serve_session(&target, Some(info), Some(elf), serve)?;
        }
        Command::Attach { probe, elf, serve } => {
            probe.no_session("attach")?;
            let info = elf.as_deref().map(parse_elf).transpose()?;
            let target = probe.attach(false)?;
            serve_session(&target, info, elf, serve)?;
        }
        Command::Read { target, locations } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, words("read", locations));
            }
            let (probe, elf) = target.attach()?;
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
            if let Some(session) = target.probe.session()? {
                let mut args = words("watch", locations);
                args.extend(["--interval".into(), interval.to_string()]);
                if let Some(count) = count {
                    args.extend(["--count".into(), count.to_string()]);
                }
                return daemon::request(session, args);
            }
            let (probe, elf) = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            let locations = locations
                .iter()
                .map(|location| path::locate(elf.as_ref(), &mut memory, location))
                .collect::<Result<Vec<_>>>()?;
            let interval = Duration::from_millis(interval);
            watch(elf.as_ref(), &mut memory, &locations, interval, count)?;
        }
        Command::Tasks { target, duration } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(
                    session,
                    words("tasks", ["--duration".into(), duration.to_string()]),
                );
            }
            let (probe, elf) = target.attach()?;
            let elf = elf.ok_or_else(|| anyhow!("pass --elf to find the tasks"))?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            print!(
                "{}",
                tasks::profile(&elf, &mut memory, Duration::from_millis(duration))?
            );
        }
        Command::Values { target } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, words("values", []));
            }
            let (probe, elf) = target.attach()?;
            let elf = elf.ok_or_else(|| anyhow!("pass --elf to find the values"))?;
            let mut session = probe.session.lock();
            print!("{}", api::values(&elf, &mut probe.memory(&mut session)?)?);
        }
        Command::Set {
            target,
            location,
            value,
        } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, words("set", [location, value]));
            }
            let (probe, elf) = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            let location = path::locate(elf.as_ref(), &mut memory, &location)?;
            print!("{}", set(elf.as_ref(), &mut memory, &location, &value)?);
            print!("{}", read(elf.as_ref(), &mut memory, &location)?);
        }
        Command::Halt { target } => run_control(&target, "halt", debug::halt)?,
        Command::Resume { target } => run_control(&target, "resume", debug::resume)?,
        Command::Step { target } => run_control(&target, "step", debug::step)?,
        Command::Status { target } => run_control(&target, "status", |_| Ok(()))?,
        Command::Break { probe, target } => {
            let session = daemon::find(probe.as_deref())?.ok_or_else(no_session_for_breakpoints)?;
            daemon::request(session, words("break", [target]))?;
        }
        Command::Delete { probe, ids } => {
            let session = daemon::find(probe.as_deref())?.ok_or_else(no_session_for_breakpoints)?;
            daemon::request(session, words("delete", ids.iter().map(u32::to_string)))?;
        }
        Command::Flash {
            probe,
            elf,
            verify,
            chip_erase,
            halt,
        } => {
            if let Some(session) = probe.session()? {
                let mut args = words("flash", [daemon::absolute(&elf)?]);
                for (set, flag) in [
                    (verify, "--verify"),
                    (chip_erase, "--chip-erase"),
                    (halt, "--halt"),
                ] {
                    if set {
                        args.push(flag.into());
                    }
                }
                return daemon::request(session, args);
            }
            let target = probe.attach(false)?;
            let mut session = target.session.lock();
            flash::flash(&mut session, &elf, verify, chip_erase)?;
            flash::reset(&mut session, halt)?;
        }
        Command::Reset { probe, halt } => {
            if let Some(session) = probe.session()? {
                let mut args = words("reset", []);
                if halt {
                    args.push("--halt".into());
                }
                return daemon::request(session, args);
            }
            let target = probe.attach(false)?;
            flash::reset(&mut target.session.lock(), halt)?;
        }
        Command::Erase { probe } => {
            probe.no_session("erase")?;
            flash::erase(&mut probe.attach(true)?.session.lock())?;
        }
        Command::List => target::list(),
    }
    Ok(())
}

fn serve_session(
    target: &target::Target,
    info: Option<ElfInfo>,
    elf: Option<PathBuf>,
    serve: ServeArgs,
) -> Result<()> {
    let options = session::Options {
        elf,
        gdb: serve.gdb,
        interactive: serve.interactive,
    };
    session::run(target, info, options)
}

fn no_session_for_breakpoints() -> anyhow::Error {
    anyhow!(
        "breakpoints need a running `run` or `attach`, since probe-rs clears them when it \
         detaches"
    )
}

/// Halt, resume, step or print the status, through the session if one runs. Without
/// one, the core keeps the state `action` leaves when the probe detaches.
fn run_control(
    target: &TargetArgs,
    command: &str,
    action: fn(&mut probe_rs::Session) -> Result<()>,
) -> Result<()> {
    if let Some(session) = target.probe.session()? {
        return daemon::request(session, words(command, []));
    }
    let (probe, elf) = target.attach()?;
    let mut session = probe.session.lock();
    action(&mut session)?;
    let status = debug::status(&mut session)?;
    let breakpoints = debug::Breakpoints::default();
    println!(
        "{}",
        debug::describe_status(elf.as_ref(), &mut session, &breakpoints, status)?
    );
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
        if interval.is_zero() {
            next = now;
        } else if next < now {
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
        (Location::Node(node), Some(elf)) => match api::kind(node) {
            Some(api::Kind::Watch) => api::render_watch(elf, memory, node),
            Some(api::Kind::Param) => api::render_param(elf, memory, node),
            None => value::render(elf, memory, node),
        },
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

/// Write `text` to `location`, and return any notes about the result.
fn set(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    location: &Location,
    text: &str,
) -> Result<String> {
    if let (Location::Node(node), Some(elf)) = (location, elf) {
        match api::kind(node) {
            Some(api::Kind::Param) => return api::request(elf, memory, node, text),
            Some(api::Kind::Watch) => bail!(
                "{} is a `watch!` value, which only the firmware writes",
                node.path
            ),
            None => {}
        }
        if let Some((kind, owner)) = api::owner(elf, memory, node)? {
            bail!(
                "{} is inside {}; {}",
                node.path,
                owner.path,
                match kind {
                    api::Kind::Param =>
                        format!("set {} itself, so the firmware applies it", owner.path),
                    api::Kind::Watch => "only the firmware writes a `watch!` value".into(),
                }
            );
        }
    }
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
        return Ok(
            "warning: the value read back differs from the value written. The firmware \
                   may have changed it since, or this memory ignores writes.\n"
                .into(),
        );
    }
    Ok(String::new())
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

#[cfg(test)]
mod api_tests {
    use super::*;
    use memory::Fake;

    #[test]
    fn refuses_writes_around_the_firmware() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tune_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        let mut memory = Fake::from_elf(path);
        for (location, expected) in [
            ("motor.speed", "only the firmware writes"),
            ("tune_lab::KP.value", "set tune_lab::KP itself"),
            ("tune_lab::KP.max", "set tune_lab::KP itself"),
        ] {
            let location = path::locate(Some(&elf), &mut memory, location).unwrap();
            let error = set(Some(&elf), &mut memory, &location, "1")
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }
}
