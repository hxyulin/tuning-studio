mod action;
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
mod trace;
mod value;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use studio_dwarf::tree::{self, NodeKind, Step};
use studio_dwarf::{ElfInfo, ElfParser};

use action::{
    Action, BreakArgs, DeleteArgs, FlashArgs, ReadArgs, ResetArgs, SetArgs, TasksArgs, TraceArgs,
    WatchArgs,
};
use flash::AfterReset;
use memory::Memory;
use path::Location;
use probe_rs::Permissions;

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
        #[command(flatten)]
        args: ReadArgs,
    },
    /// Read variables or addresses repeatedly until Ctrl-C
    Watch {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        args: WatchArgs,
    },
    /// Sample every Embassy task while the firmware runs, then print how often
    /// each was run-queued and which `.await` it was parked on
    Tasks {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        args: TasksArgs,
    },
    /// Record every write to up to four values with the DWT's data trace,
    /// without halting the core, then print each with its time
    Trace {
        #[command(flatten)]
        target: TargetArgs,
        #[command(flatten)]
        args: TraceArgs,
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
        #[command(flatten)]
        args: SetArgs,
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
        #[command(flatten)]
        args: BreakArgs,
    },
    /// Clear the breakpoints with these numbers, or all of them
    Delete {
        /// Probe of the session, needed when several are running
        #[arg(long)]
        probe: Option<String>,
        #[command(flatten)]
        args: DeleteArgs,
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
        #[command(flatten)]
        options: FlashArgs,
    },
    /// Reset the target
    Reset {
        #[command(flatten)]
        probe: ProbeArgs,
        #[command(flatten)]
        args: ResetArgs,
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
    fn attach(&self) -> Result<target::Target> {
        self.attach_with(Permissions::default())
    }

    /// Attach with permission to erase the whole chip.
    fn attach_for_erase(&self) -> Result<target::Target> {
        self.attach_with(Permissions::new().allow_erase_all())
    }

    fn attach_with(&self, permissions: Permissions) -> Result<target::Target> {
        let chip = self
            .chip
            .as_deref()
            .ok_or_else(|| anyhow!("--chip is needed when no `run` or `attach` is running"))?;
        target::Target::attach(chip, self.probe.as_deref(), self.speed, permissions)
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
        Ok((self.probe.attach()?, elf))
    }
}

fn parse_elf(path: &Path) -> Result<ElfInfo> {
    ElfParser::parse(path).with_context(|| format!("could not parse {}", path.display()))
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
            let target = probe.attach()?;
            {
                let mut session = target.session.lock();
                let options = FlashArgs {
                    verify,
                    chip_erase: false,
                    halt: false,
                };
                flash::flash(&mut session, &elf, &options)?;
                if let Some(mut logs) = logs::Logs::new(&elf)? {
                    logs.restart(&mut session)?;
                }
                flash::reset(&mut session, AfterReset::Run)?;
            }
            serve_session(&target, Some(info), Some(elf), serve)?;
        }
        Command::Attach { probe, elf, serve } => {
            probe.no_session("attach")?;
            let info = elf.as_deref().map(parse_elf).transpose()?;
            let target = probe.attach()?;
            serve_session(&target, info, elf, serve)?;
        }
        Command::Read { target, args } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, Action::Read(args).to_words());
            }
            let (probe, elf) = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            print!(
                "{}",
                read_locations(elf.as_ref(), &mut memory, &args.locations)?
            );
        }
        Command::Watch { target, args } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, Action::Watch(args).to_words());
            }
            let (probe, elf) = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            let locations = locate_all(elf.as_ref(), &mut memory, &args.locations)?;
            let interval = Duration::from_millis(args.interval);
            watch(elf.as_ref(), &mut memory, &locations, interval, args.count)?;
        }
        Command::Tasks { target, args } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, Action::Tasks(args).to_words());
            }
            let (probe, elf) = target.attach()?;
            let elf = elf.ok_or_else(|| anyhow!("pass --elf to find the tasks"))?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            print!(
                "{}",
                tasks::profile(&elf, &mut memory, Duration::from_millis(args.duration))?
            );
        }
        Command::Trace { target, args } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, Action::Trace(args).to_words());
            }
            // Ctrl-C ends collection early; the ETF is still stopped and reported.
            let stop = Arc::new(AtomicBool::new(false));
            let handler = Arc::clone(&stop);
            ctrlc::set_handler(move || handler.store(true, Ordering::Relaxed))?;
            session::block_sigint();
            let (probe, elf) = target.attach()?;
            let mut session = probe.session.lock();
            print!(
                "{}",
                trace::run(
                    &probe,
                    &mut session,
                    elf.as_ref(),
                    &args.locations,
                    Duration::from_millis(args.duration),
                    &stop
                )?
            );
        }
        Command::Values { target } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, Action::Values.to_words());
            }
            let (probe, elf) = target.attach()?;
            let elf = elf.ok_or_else(|| anyhow!("pass --elf to find the values"))?;
            let mut session = probe.session.lock();
            print!("{}", api::values(&elf, &mut probe.memory(&mut session)?)?);
        }
        Command::Set { target, args } => {
            if let Some(session) = target.probe.session()? {
                return daemon::request(session, Action::Set(args).to_words());
            }
            let (probe, elf) = target.attach()?;
            let mut session = probe.session.lock();
            let mut memory = probe.memory(&mut session)?;
            print!(
                "{}",
                set_and_read(elf.as_ref(), &mut memory, &args.location, &args.value)?
            );
        }
        Command::Halt { target } => run_control(&target, Action::Halt, debug::halt)?,
        Command::Resume { target } => run_control(&target, Action::Resume, debug::resume)?,
        Command::Step { target } => run_control(&target, Action::Step, debug::step)?,
        Command::Status { target } => run_control(&target, Action::Status, |_| Ok(()))?,
        Command::Break { probe, args } => {
            let session = daemon::find(probe.as_deref())?.ok_or_else(no_session_for_breakpoints)?;
            daemon::request(session, Action::Break(args).to_words())?;
        }
        Command::Delete { probe, args } => {
            let session = daemon::find(probe.as_deref())?.ok_or_else(no_session_for_breakpoints)?;
            daemon::request(session, Action::Delete(args).to_words())?;
        }
        Command::Flash {
            probe,
            elf,
            options,
        } => {
            if let Some(session) = probe.session()? {
                // The session may run in another directory.
                let elf = PathBuf::from(daemon::absolute(&elf)?);
                let action = Action::Flash {
                    elf: Some(elf),
                    options,
                };
                return daemon::request(session, action.to_words());
            }
            let target = probe.attach()?;
            let mut session = target.session.lock();
            flash::flash(&mut session, &elf, &options)?;
            flash::reset(&mut session, options.after_reset())?;
        }
        Command::Reset { probe, args } => {
            if let Some(session) = probe.session()? {
                return daemon::request(session, Action::Reset(args).to_words());
            }
            let target = probe.attach()?;
            flash::reset(&mut target.session.lock(), args.after_reset())?;
        }
        Command::Erase { probe } => {
            probe.no_session("erase")?;
            flash::erase(&mut probe.attach_for_erase()?.session.lock())?;
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
    command: Action,
    action: fn(&mut probe_rs::Session) -> Result<()>,
) -> Result<()> {
    if let Some(session) = target.probe.session()? {
        return daemon::request(session, command.to_words());
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
        let text = read_all(elf, memory, &[locations]).remove(0)?;
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

/// Read each group of locations once, as one output each. The values they show
/// are fetched first, values close together in one transfer, so reading several
/// costs little more than reading one.
pub fn read_all(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    groups: &[&[Location]],
) -> Vec<Result<String>> {
    let groups: Vec<Result<Vec<Location>>> = groups
        .iter()
        .map(|group| group.iter().map(|l| refresh(elf, memory, l)).collect())
        .collect();
    let mut spans = Vec::new();
    for location in groups.iter().flatten().flatten() {
        // A location that fails here fails again when it is read, with its error.
        if let Ok(Some(span)) = span(elf, memory, location) {
            spans.push(span);
        }
    }
    // Without the prefetch, each value reads its own bytes and reports its own error.
    if memory::prefetch(memory, spans).is_err() {
        memory.forget();
    }
    let texts = groups
        .into_iter()
        .map(|group| {
            group?.iter().try_fold(String::new(), |text, location| {
                Ok(text + &read(elf, memory, location)?)
            })
        })
        .collect();
    memory.forget();
    texts
}

/// Read each location once, together as [`read_all`] does, stopping at the first
/// that fails.
pub fn read_each(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    locations: &[Location],
) -> Result<String> {
    let groups: Vec<&[Location]> = locations.iter().map(std::slice::from_ref).collect();
    read_all(elf, memory, &groups).into_iter().collect()
}

/// Resolve every location text, stopping at the first that fails.
fn locate_all(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    texts: &[String],
) -> Result<Vec<Location>> {
    texts
        .iter()
        .map(|text| path::locate(elf, memory, text))
        .collect()
}

/// Resolve and read each location once, for `read` with or without a session.
pub fn read_locations(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    texts: &[String],
) -> Result<String> {
    let locations = locate_all(elf, memory, texts)?;
    read_each(elf, memory, &locations)
}

/// Write one value and read it back, for `set` with or without a session.
pub fn set_and_read(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    text: &str,
    value: &str,
) -> Result<String> {
    let location = path::locate(elf, memory, text)?;
    let mut out = set(elf, memory, &location, value)?;
    out += &read(elf, memory, &location)?;
    Ok(out)
}

/// The bytes [`read`] reads of `location` at once. A raw address has none, so the
/// bytes around it, which may be registers, are never read.
fn span(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    location: &Location,
) -> Result<Option<(u64, usize)>> {
    Ok(match (location, elf) {
        (Location::Node(node), Some(elf)) => match api::kind(node) {
            Some(kind) => Some(api::span(elf, memory, kind, node)?),
            None => value::span(node),
        },
        _ => None,
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
    fn watch_count_must_be_positive() {
        assert!(Cli::try_parse_from(["tuning", "watch", "0x2000_0000", "--count", "0"]).is_err());
        assert!(Cli::try_parse_from(["tuning", "watch", "0x2000_0000", "--count", "2"]).is_ok());
    }

    fn parse(args: &[&str]) -> Command {
        let args = ["tuning"].into_iter().chain(args.iter().copied());
        Cli::try_parse_from(args).unwrap().command
    }

    // These are the forms main() forwards to a running session.
    #[test]
    fn parses_the_commands_a_session_serves() {
        let Command::Watch { args, .. } = parse(&["watch", "a", "--elf", "fw.elf", "--count", "4"])
        else {
            panic!("not watch");
        };
        assert_eq!((args.interval, args.count), (100, Some(4)));
        let Command::Tasks { args, .. } = parse(&["tasks"]) else {
            panic!("not tasks");
        };
        assert_eq!(args.duration, 1000);
        let Command::Trace { args, .. } = parse(&["trace", "a", "--duration", "5"]) else {
            panic!("not trace");
        };
        assert_eq!(args.duration, 5);
        let Command::Set { args, .. } = parse(&["set", "a", "-2"]) else {
            panic!("not set");
        };
        assert_eq!(args.value, "-2");
        let Command::Flash { options, .. } =
            parse(&["flash", "fw.elf", "--verify", "--chip-erase", "--halt"])
        else {
            panic!("not flash");
        };
        assert!(options.verify && options.chip_erase && options.halt);
        let Command::Reset { args, .. } = parse(&["reset", "--halt"]) else {
            panic!("not reset");
        };
        assert!(args.halt);
        assert!(matches!(parse(&["break", "main"]), Command::Break { .. }));
        let Command::Delete { args, .. } = parse(&["delete", "2"]) else {
            panic!("not delete");
        };
        assert_eq!(args.ids, [2]);
    }

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

    #[test]
    fn reads_nearby_values_in_one_transfer() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tune_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        let mut memory = Fake::from_elf(path);
        let texts = ["motor.error", "motor.speed", "motor.state", "tune_lab::KP"];
        let locations = texts
            .iter()
            .map(|text| path::locate(Some(&elf), &mut memory, text).unwrap())
            .collect::<Vec<_>>();
        // The first read also fetches the units from flash.
        read_each(Some(&elf), &mut memory, &locations).unwrap();
        memory.transfers = 0;
        let text = read_each(Some(&elf), &mut memory, &locations).unwrap();
        assert_eq!(memory.transfers, 1, "{text}");
        assert!(
            text.starts_with("motor.error: f32 = <not written yet>\n"),
            "{text}"
        );
    }
}
