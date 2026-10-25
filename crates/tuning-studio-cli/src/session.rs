//! One attachment that takes a command per line on stdin. The main thread owns the
//! probe: it runs each command, prints due watches and RTT logs, reports when the
//! core halts or resumes, and drives the GDB server. A second thread only reads stdin.
//!
//! The session ends on `quit` or Ctrl-C, and on end of input unless it streams logs.
//! It resumes a halted core, and detaching clears the breakpoints.

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use probe_rs::{CoreStatus, VectorCatchCondition};
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use studio_dwarf::{ElfInfo, ElfParser};

use crate::debug::{self, Breakpoints};
use crate::flash;
use crate::gdb;
use crate::logs::Logs;
use crate::path::{self, Location};
use crate::target::Target;

/// How often the session checks whether the core has halted
const STATUS_INTERVAL: Duration = Duration::from_millis(50);
/// How often the session reads RTT
const LOG_INTERVAL: Duration = Duration::from_millis(10);

pub struct Options {
    /// Firmware ELF, for symbols, logs and `flash`
    pub elf: Option<PathBuf>,
    /// Serve GDB on this address
    pub gdb: Option<SocketAddr>,
    /// Print RTT logs, halt on a HardFault so its cause stays visible, and keep
    /// running after the end of input
    pub logs: bool,
}

#[derive(Parser)]
#[command(
    name = "",
    no_binary_name = true,
    disable_version_flag = true,
    help_template = "{subcommands}"
)]
struct Line {
    #[command(subcommand)]
    command: SessionCommand,
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Read variables or addresses once
    Read {
        #[arg(required = true)]
        locations: Vec<String>,
    },
    /// Read variables or addresses repeatedly, until `unwatch`
    Watch {
        #[arg(required = true)]
        locations: Vec<String>,
        /// Time between reads in milliseconds
        #[arg(long, default_value_t = 100)]
        interval: u64,
        /// Stop after this many reads
        #[arg(long)]
        count: Option<u64>,
    },
    /// Stop the watches with these numbers, or all of them
    Unwatch { ids: Vec<u32> },
    /// Write one value, then read it back
    Set {
        location: String,
        #[arg(allow_negative_numbers = true)]
        value: String,
    },
    /// Halt the core
    Halt,
    /// Resume the core
    Resume,
    /// Execute one instruction on a halted core
    Step,
    /// Set a hardware breakpoint on a function or an address
    Break {
        /// Function name, such as `SysTick` or `control::tick`, or an address
        target: String,
    },
    /// Clear the breakpoints with these numbers, or all of them
    Delete { ids: Vec<u32> },
    /// Reset the core and let it run
    Reset {
        /// Halt at the reset vector instead
        #[arg(long)]
        halt: bool,
    },
    /// Program the ELF again, which may have been rebuilt, then reset
    Flash {
        /// Read the flash back afterwards
        #[arg(long)]
        verify: bool,
    },
    /// Print the core status, breakpoints and watches
    Status,
    /// Detach and exit
    Quit,
}

struct Watch {
    id: u32,
    texts: Vec<String>,
    locations: Vec<Location>,
    interval: Duration,
    next: Instant,
    remaining: Option<u64>,
}

struct State {
    elf_path: Option<PathBuf>,
    elf: Option<ElfInfo>,
    logs: Option<Logs>,
    start: Instant,
    watches: Vec<Watch>,
    next_watch: u32,
    breakpoints: Breakpoints,
    status: Option<CoreStatus>,
}

pub fn run(target: &Target, elf: Option<ElfInfo>, options: Options) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let handler = Arc::clone(&stop);
    ctrlc::set_handler(move || handler.store(true, Ordering::Relaxed))?;
    let (lines, input) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    // A signal that lands during a USB transfer makes macOS fail the transfer, and
    // the probe stays out of step afterwards. Block SIGINT here so one of the threads
    // above takes it; they inherited the mask from before this call.
    block_sigint();

    let mut server = options
        .gdb
        .map(|address| gdb::Server::new(&target.session, vec![0], &[address]))
        .transpose()
        .map_err(|e| anyhow!("could not start the GDB server: {e:#}"))?;
    if let Some(address) = options.gdb {
        println!("GDB server listening on {address}");
    }
    let logs = match (&options.elf, options.logs) {
        (Some(path), true) => Logs::new(path)?,
        _ => None,
    };
    if options.logs {
        target
            .session
            .lock()
            .core(0)?
            .enable_vector_catch(VectorCatchCondition::HardFault)?;
    }
    if options.logs && logs.is_none() {
        println!("the ELF has no RTT control block, so there are no logs to show");
    }
    let mut state = State {
        elf_path: options.elf,
        elf,
        logs,
        start: Instant::now(),
        watches: Vec::new(),
        next_watch: 0,
        breakpoints: Breakpoints::default(),
        status: None,
    };
    let mut next_status = Instant::now();
    let mut next_logs = Instant::now();
    let mut input_open = true;

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        // Logs first, so a panic message prints before the halt it causes.
        if let Some(logs) = &mut state.logs
            && now >= next_logs
        {
            let text = logs.poll(&mut target.session.lock());
            match text {
                Ok(text) => print!("{text}"),
                Err(e) => println!("error: could not read RTT: {e:#}"),
            }
            next_logs = now + LOG_INTERVAL;
        }
        if now >= next_status {
            report_status(target, &mut state)?;
            next_status = now + STATUS_INTERVAL;
        }
        run_watches(target, &mut state);
        let mut wake = state
            .watches
            .iter()
            .map(|w| w.next)
            .fold(next_status, Instant::min);
        if state.logs.is_some() {
            wake = wake.min(next_logs);
        }
        if let Some(server) = &mut server {
            let connected = server.is_connected();
            let wait = server
                .process()
                .map_err(|e| anyhow!("GDB server failed: {e:#}"))?;
            match (connected, server.is_connected()) {
                (false, true) => println!("GDB connected"),
                // GDB expects `detach` to resume the program; the probe-rs stub does not.
                (true, false) => {
                    let mut session = target.session.lock();
                    if matches!(debug::status(&mut session)?, CoreStatus::Halted(_)) {
                        debug::resume(&mut session)?;
                        state.status = Some(CoreStatus::Running);
                        println!("GDB disconnected; resumed the core");
                    } else {
                        println!("GDB disconnected");
                    }
                }
                _ => {}
            }
            wake = wake.min(Instant::now() + wait);
        }
        std::io::stdout().flush()?;

        let timeout = wake.saturating_duration_since(Instant::now());
        if !input_open {
            std::thread::sleep(timeout);
            continue;
        }
        match input.recv_timeout(timeout) {
            Ok(line) => {
                let line = line?;
                match handle(target, &mut state, &line) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => println!("error: {e:#}"),
                }
                // Report a halt or resume the command caused before anything else.
                next_status = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            // Logs keep streaming without input, as `cargo run` with no terminal needs.
            Err(RecvTimeoutError::Disconnected) if state.logs.is_some() => input_open = false,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // Detaching leaves a halted core halted. Clear the breakpoints first, including
    // GDB's, or the core can stop at one again before probe-rs clears them.
    let mut session = target.session.lock();
    session.clear_all_hw_breakpoints()?;
    if matches!(debug::status(&mut session)?, CoreStatus::Halted(_)) {
        debug::resume(&mut session)?;
        println!("resumed the core before detaching");
    }
    Ok(())
}

/// Run one input line. Returns whether the session should end.
fn handle(target: &Target, state: &mut State, line: &str) -> Result<bool> {
    let words = shlex::split(line).ok_or_else(|| anyhow!("unbalanced quotes"))?;
    if words.is_empty() {
        return Ok(false);
    }
    let command = match Line::try_parse_from(words) {
        Ok(line) => line.command,
        Err(e) => {
            // Help and parse errors both go to stdout, so a client reads one stream.
            // The usage hint names a binary and `--help`, which mean nothing here.
            let text = e.render().to_string();
            let text = text
                .split("\n\nFor more information")
                .next()
                .unwrap_or_default();
            println!("{}", text.trim_end());
            return Ok(false);
        }
    };
    let mut session = target.session.lock();
    match command {
        SessionCommand::Reset { halt } => {
            if let Some(logs) = &mut state.logs {
                logs.restart(&mut session)?;
            }
            flash::reset(&mut session, halt)?;
            state.status = None;
            return Ok(false);
        }
        SessionCommand::Flash { verify } => {
            let path = state
                .elf_path
                .clone()
                .ok_or_else(|| anyhow!("flash needs an ELF"))?;
            reflash(target, &mut session, state, &path, verify)?;
            return Ok(false);
        }
        _ => {}
    }
    let elf = state.elf.as_ref();
    match command {
        SessionCommand::Reset { .. } | SessionCommand::Flash { .. } => unreachable!(),
        SessionCommand::Read { locations } => {
            let mut memory = target.memory(&mut session)?;
            for location in &locations {
                let location = path::locate(elf, &mut memory, location)?;
                print!("{}", crate::read(elf, &mut memory, &location)?);
            }
        }
        SessionCommand::Watch {
            locations,
            interval,
            count,
        } => {
            let mut memory = target.memory(&mut session)?;
            let resolved = locations
                .iter()
                .map(|location| path::locate(elf, &mut memory, location))
                .collect::<Result<Vec<_>>>()?;
            state.next_watch += 1;
            let watch = Watch {
                id: state.next_watch,
                texts: locations,
                locations: resolved,
                interval: Duration::from_millis(interval.max(1)),
                next: Instant::now(),
                remaining: count,
            };
            println!(
                "watch {}: {} every {} ms",
                watch.id,
                watch.texts.join(" "),
                watch.interval.as_millis()
            );
            state.watches.push(watch);
        }
        SessionCommand::Unwatch { ids } => {
            if let Some(id) = ids
                .iter()
                .find(|id| !state.watches.iter().any(|w| w.id == **id))
            {
                return Err(anyhow!("no watch {id}"));
            }
            state.watches.retain(|w| {
                let keep = !ids.is_empty() && !ids.contains(&w.id);
                if !keep {
                    println!("watch {} stopped", w.id);
                }
                keep
            });
        }
        SessionCommand::Set { location, value } => {
            let mut memory = target.memory(&mut session)?;
            let location = path::locate(elf, &mut memory, &location)?;
            crate::set(elf, &mut memory, &location, &value)?;
            print!("{}", crate::read(elf, &mut memory, &location)?);
        }
        SessionCommand::Halt => debug::halt(&mut session)?,
        SessionCommand::Resume => {
            debug::resume(&mut session)?;
            // The core may halt at a breakpoint again before the next poll, which would
            // otherwise look like no change.
            state.status = Some(CoreStatus::Running);
            println!("running");
        }
        SessionCommand::Step => {
            debug::step(&mut session)?;
            // A step leaves the core halted, so the status does not change; print it here.
            let status = debug::status(&mut session)?;
            state.status = Some(status);
            let text = debug::describe_status(elf, &mut session, &state.breakpoints, status)?;
            println!("{text}");
        }
        SessionCommand::Break { target: text } => {
            let breakpoint = state.breakpoints.add(elf, &mut session, &text)?;
            println!("breakpoint {} at {}", breakpoint.id, breakpoint.label);
        }
        SessionCommand::Delete { ids } => {
            for breakpoint in state.breakpoints.delete(&mut session, &ids)? {
                println!("breakpoint {} cleared", breakpoint.id);
            }
        }
        SessionCommand::Status => {
            let status = debug::status(&mut session)?;
            println!(
                "{}",
                debug::describe_status(elf, &mut session, &state.breakpoints, status)?
            );
            for breakpoint in state.breakpoints.iter() {
                println!("breakpoint {} at {}", breakpoint.id, breakpoint.label);
            }
            for watch in &state.watches {
                println!(
                    "watch {}: {} every {} ms",
                    watch.id,
                    watch.texts.join(" "),
                    watch.interval.as_millis()
                );
            }
        }
        SessionCommand::Quit => return Ok(true),
    }
    Ok(false)
}

/// Print the core status when it changes.
fn report_status(target: &Target, state: &mut State) -> Result<()> {
    let mut session = target.session.lock();
    let status = debug::status(&mut session)?;
    if state.status != Some(status) {
        state.status = Some(status);
        let text =
            debug::describe_status(state.elf.as_ref(), &mut session, &state.breakpoints, status)?;
        println!("{text}");
    }
    Ok(())
}

/// Print every due watch. A watch that fails prints the error and stops.
fn run_watches(target: &Target, state: &mut State) {
    let now = Instant::now();
    if !state.watches.iter().any(|w| w.next <= now) {
        return;
    }
    let elf = state.elf.as_ref();
    let elapsed = state.start.elapsed().as_secs_f64();
    let mut session = target.session.lock();
    let mut memory = match target.memory(&mut session) {
        Ok(memory) => memory,
        Err(e) => {
            println!("error: {e:#}");
            return;
        }
    };
    state.watches.retain_mut(|watch| {
        if watch.next > now {
            return true;
        }
        let text = watch
            .locations
            .iter()
            .try_fold(String::new(), |text, location| {
                let location = crate::refresh(elf, &mut memory, location)?;
                Ok::<_, anyhow::Error>(text + &crate::read(elf, &mut memory, &location)?)
            });
        match text {
            Ok(text) => print!("--- watch {}, {elapsed:.3} s\n{text}", watch.id),
            Err(e) => {
                println!("error: watch {} stopped: {e:#}", watch.id);
                return false;
            }
        }
        // Keep to the schedule; after a slow read, skip the reads it overran.
        watch.next += watch.interval;
        while watch.next <= now {
            watch.next += watch.interval;
        }
        watch.remaining = watch.remaining.map(|n| n - 1);
        if watch.remaining == Some(0) {
            println!("watch {} done", watch.id);
            return false;
        }
        true
    });
}

/// Program `path`, reload its symbols and logs, and reset. Breakpoints are cleared,
/// since the functions may have moved, and watches are resolved again.
fn reflash(
    target: &Target,
    session: &mut probe_rs::Session,
    state: &mut State,
    file: &std::path::Path,
    verify: bool,
) -> Result<()> {
    let elf =
        ElfParser::parse(file).with_context(|| format!("could not parse {}", file.display()))?;
    let logs = if state.logs.is_some() {
        Logs::new(file)?
    } else {
        None
    };
    for breakpoint in state.breakpoints.delete(session, &[])? {
        println!("breakpoint {} cleared", breakpoint.id);
    }
    flash::flash(session, file, verify, false)?;
    state.elf = Some(elf);
    state.logs = logs;
    if let Some(logs) = &mut state.logs {
        logs.restart(session)?;
    }
    flash::reset(session, false)?;
    state.status = None;
    let mut memory = target.memory(session)?;
    let elf = state.elf.as_ref();
    state.watches.retain_mut(|watch| {
        let located = watch
            .texts
            .iter()
            .map(|text| path::locate(elf, &mut memory, text))
            .collect::<Result<Vec<_>>>();
        match located {
            Ok(locations) => {
                watch.locations = locations;
                true
            }
            Err(e) => {
                println!("error: watch {} stopped: {e:#}", watch.id);
                false
            }
        }
    });
    Ok(())
}

/// Block SIGINT on the calling thread. Call it after `ctrlc::set_handler`, whose
/// thread then takes the signal.
pub fn block_sigint() {
    // SAFETY: the set is initialised by sigemptyset before use, and
    // pthread_sigmask only changes this thread's mask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}
