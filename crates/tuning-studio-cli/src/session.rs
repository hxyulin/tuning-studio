//! One attachment that takes a command per line on stdin. The main thread owns the
//! probe: it runs each command, prints due watches, reports when the core halts or
//! resumes, and drives the GDB server. A second thread only reads stdin.
//!
//! The session ends on `quit`, end of input or Ctrl-C. It resumes a halted core,
//! and detaching clears the breakpoints.

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use probe_rs::CoreStatus;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use studio_dwarf::ElfInfo;

use crate::debug::{self, Breakpoints};
use crate::gdb;
use crate::path::{self, Location};
use crate::target::Target;

/// How often the session checks whether the core has halted
const STATUS_INTERVAL: Duration = Duration::from_millis(50);

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
    /// Print the core status, breakpoints and watches
    Status,
    /// Detach and exit
    Quit,
}

struct Watch {
    id: u32,
    text: String,
    locations: Vec<Location>,
    interval: Duration,
    next: Instant,
    remaining: Option<u64>,
}

struct State<'e> {
    elf: Option<&'e ElfInfo>,
    start: Instant,
    watches: Vec<Watch>,
    next_watch: u32,
    breakpoints: Breakpoints,
    status: Option<CoreStatus>,
}

pub fn run(target: &Target, elf: Option<&ElfInfo>, gdb_address: Option<SocketAddr>) -> Result<()> {
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

    let mut server = gdb_address
        .map(|address| gdb::Server::new(&target.session, vec![0], &[address]))
        .transpose()
        .map_err(|e| anyhow!("could not start the GDB server: {e:#}"))?;
    if let Some(address) = gdb_address {
        println!("GDB server listening on {address}");
    }
    let mut state = State {
        elf,
        start: Instant::now(),
        watches: Vec::new(),
        next_watch: 0,
        breakpoints: Breakpoints::default(),
        status: None,
    };
    let mut next_status = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
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
    let elf = state.elf;
    let mut session = target.session.lock();
    match command {
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
                text: locations.join(" "),
                locations: resolved,
                interval: Duration::from_millis(interval.max(1)),
                next: Instant::now(),
                remaining: count,
            };
            println!(
                "watch {}: {} every {} ms",
                watch.id,
                watch.text,
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
                    watch.text,
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
        let text = debug::describe_status(state.elf, &mut session, &state.breakpoints, status)?;
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
    let elf = state.elf;
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
