//! One attachment that serves commands from the socket, and from stdin with
//! `--interactive`. The main thread owns the probe: it runs each command, prints
//! due watches and RTT logs, reports when the core halts or resumes, and drives the
//! GDB server. Other threads only read stdin and the socket.
//!
//! A command's output goes back to whoever sent it. Logs, halts and GDB events go
//! to this process's stdout.
//!
//! The session ends on `quit` from stdin or on Ctrl-C. It resumes a halted core,
//! and detaching clears the breakpoints.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use probe_rs::{CoreStatus, Session, VectorCatchCondition};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use studio_dwarf::{ElfInfo, ElfParser};

use crate::daemon::{self, Request, Response};
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
/// A client that cannot take output this long is dropped.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

pub struct Options {
    /// Firmware ELF, for symbols, logs and `flash`
    pub elf: Option<PathBuf>,
    /// Serve GDB on this address
    pub gdb: Option<SocketAddr>,
    /// Also take commands from stdin
    pub interactive: bool,
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
    /// Print every `Param` and `watch!` value
    Values,
    /// Write one value, then read it back. A `Param` changes through the
    /// firmware, which applies its range and step.
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
    /// Program an ELF, by default the session's, which may have been rebuilt; then
    /// reset. The session uses the new ELF's symbols and logs from then on.
    Flash {
        elf: Option<PathBuf>,
        /// Read the flash back afterwards
        #[arg(long)]
        verify: bool,
        /// Erase the whole chip rather than only the sectors the ELF uses
        #[arg(long)]
        chip_erase: bool,
        /// Halt at the reset vector instead of running
        #[arg(long)]
        halt: bool,
    },
    /// Print the core status, breakpoints and watches
    Status,
    /// Detach and exit
    Quit,
}

/// Where a command's output goes
#[derive(Clone, Copy, PartialEq)]
enum Sink {
    Stdout,
    Client(u32),
}

enum Input {
    Stdin(String),
    Connected(u32, UnixStream),
    Request(u32, String),
    Disconnected(u32),
}

enum Outcome {
    Done,
    /// A watch now answers for the command until it ends.
    Watching,
    Quit,
}

struct Watch {
    id: u32,
    sink: Sink,
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
    clients: HashMap<u32, UnixStream>,
}

impl State {
    /// Send a response to a client. On failure the client and its watches go.
    fn send(&mut self, id: u32, response: &Response) {
        let Some(stream) = self.clients.get_mut(&id) else {
            return;
        };
        let mut line = serde_json::to_vec(response).expect("responses serialize");
        line.push(b'\n');
        if stream.write_all(&line).is_err() {
            self.drop_client(id);
        }
    }

    fn drop_client(&mut self, id: u32) {
        self.clients.remove(&id);
        self.watches.retain(|w| w.sink != Sink::Client(id));
    }

    /// Deliver a finished command's output and result.
    fn finish(&mut self, sink: Sink, output: String, error: Option<String>) {
        match sink {
            Sink::Stdout => {
                print!("{output}");
                if let Some(error) = error {
                    println!("error: {error}");
                }
            }
            Sink::Client(id) => {
                if !output.is_empty() {
                    self.send(
                        id,
                        &Response {
                            output: Some(output),
                            ..Response::default()
                        },
                    );
                }
                self.send(
                    id,
                    &Response {
                        done: true,
                        error,
                        ..Response::default()
                    },
                );
            }
        }
    }
}

pub fn run(target: &Target, elf: Option<ElfInfo>, options: Options) -> Result<()> {
    let socket = daemon::listen(&target.probe_id)?;
    let stop = Arc::new(AtomicBool::new(false));
    let handler = Arc::clone(&stop);
    ctrlc::set_handler(move || handler.store(true, Ordering::Relaxed))?;
    let (sender, input) = mpsc::channel();
    if options.interactive {
        let sender = sender.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                if sender.send(Input::Stdin(line)).is_err() {
                    break;
                }
            }
        });
    }
    let listener = socket.listener.try_clone()?;
    std::thread::spawn(move || accept(listener, sender));
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
    let logs = options.elf.as_deref().map(Logs::new).transpose()?.flatten();
    if options.elf.is_some() && logs.is_none() {
        println!("the ELF has no RTT control block, so there are no logs to show");
    }
    // Halt on a HardFault, so a panic's cause stays visible.
    target
        .session
        .lock()
        .core(0)?
        .enable_vector_catch(VectorCatchCondition::HardFault)?;
    let mut state = State {
        elf_path: options.elf,
        elf,
        logs,
        start: Instant::now(),
        watches: Vec::new(),
        next_watch: 0,
        breakpoints: Breakpoints::default(),
        status: None,
        clients: HashMap::new(),
    };
    let mut next_status = Instant::now();
    let mut next_logs = Instant::now();

    'session: while !stop.load(Ordering::Relaxed) {
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
        let (sink, words) = match input.recv_timeout(timeout) {
            Ok(Input::Stdin(line)) => match shlex::split(&line) {
                Some(words) if words.is_empty() => continue,
                Some(words) => (Sink::Stdout, words),
                None => {
                    println!("error: unbalanced quotes");
                    continue;
                }
            },
            Ok(Input::Connected(id, stream)) => {
                let _ = stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT));
                state.clients.insert(id, stream);
                continue;
            }
            Ok(Input::Request(id, line)) => match serde_json::from_str::<Request>(&line) {
                Ok(request) => (Sink::Client(id), request.args),
                Err(e) => {
                    state.finish(
                        Sink::Client(id),
                        String::new(),
                        Some(format!("bad request: {e}")),
                    );
                    continue;
                }
            },
            Ok(Input::Disconnected(id)) => {
                state.drop_client(id);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut output = String::new();
        match handle(target, &mut state, sink, words, &mut output) {
            Ok(Outcome::Done) => state.finish(sink, output, None),
            Ok(Outcome::Watching) => match sink {
                Sink::Stdout => print!("{output}"),
                Sink::Client(id) => state.send(
                    id,
                    &Response {
                        output: Some(output),
                        ..Response::default()
                    },
                ),
            },
            Ok(Outcome::Quit) => {
                state.finish(sink, output, None);
                break 'session;
            }
            Err(e) => state.finish(sink, output, Some(format!("{e:#}"))),
        }
        // Report a halt or resume the command caused before anything else.
        next_status = Instant::now();
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

/// Accept clients and forward their requests, one line each.
fn accept(listener: std::os::unix::net::UnixListener, sender: Sender<Input>) {
    for (id, stream) in (1..).zip(listener.incoming()) {
        let Ok(stream) = stream else { continue };
        let Ok(writer) = stream.try_clone() else {
            continue;
        };
        if sender.send(Input::Connected(id, writer)).is_err() {
            return;
        }
        let sender = sender.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if sender.send(Input::Request(id, line)).is_err() {
                    return;
                }
            }
            let _ = sender.send(Input::Disconnected(id));
        });
    }
}

/// Run one command and write its output to `out`.
fn handle(
    target: &Target,
    state: &mut State,
    sink: Sink,
    words: Vec<String>,
    out: &mut String,
) -> Result<Outcome> {
    let command = match Line::try_parse_from(words) {
        Ok(line) => line.command,
        Err(e) => {
            // The usage hint names a binary and `--help`, which mean nothing here.
            let text = e.render().to_string();
            let text = text
                .split("\n\nFor more information")
                .next()
                .unwrap_or_default();
            if e.use_stderr() {
                bail!("{}", text.trim_end().trim_start_matches("error: "));
            }
            writeln!(out, "{}", text.trim_end())?;
            return Ok(Outcome::Done);
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
            writeln!(out, "{}", if halt { "reset and halted" } else { "reset" })?;
            return Ok(Outcome::Done);
        }
        SessionCommand::Flash {
            elf,
            verify,
            chip_erase,
            halt,
        } => {
            let path = elf
                .or_else(|| state.elf_path.clone())
                .ok_or_else(|| anyhow!("flash needs an ELF"))?;
            reflash(
                target,
                &mut session,
                state,
                &path,
                verify,
                chip_erase,
                halt,
                out,
            )?;
            return Ok(Outcome::Done);
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
                *out += &crate::read(elf, &mut memory, &location)?;
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
                sink,
                texts: locations,
                locations: resolved,
                interval: Duration::from_millis(interval.max(1)),
                next: Instant::now(),
                remaining: count,
            };
            describe_watch(out, &watch)?;
            state.watches.push(watch);
            return Ok(Outcome::Watching);
        }
        SessionCommand::Unwatch { ids } => {
            if let Some(id) = ids
                .iter()
                .find(|id| !state.watches.iter().any(|w| w.id == **id))
            {
                bail!("no watch {id}");
            }
            let (gone, kept) = std::mem::take(&mut state.watches)
                .into_iter()
                .partition::<Vec<_>, _>(|w| ids.is_empty() || ids.contains(&w.id));
            state.watches = kept;
            for watch in gone {
                writeln!(out, "watch {} stopped", watch.id)?;
                if let Sink::Client(id) = watch.sink
                    && watch.sink != sink
                {
                    state.finish(Sink::Client(id), String::new(), Some("unwatched".into()));
                }
            }
        }
        SessionCommand::Set { location, value } => {
            let mut memory = target.memory(&mut session)?;
            let location = path::locate(elf, &mut memory, &location)?;
            *out += &crate::set(elf, &mut memory, &location, &value)?;
            *out += &crate::read(elf, &mut memory, &location)?;
        }
        SessionCommand::Values => {
            let elf = elf.ok_or_else(|| anyhow!("the session has no ELF"))?;
            *out += &crate::api::values(elf, &mut target.memory(&mut session)?)?;
        }
        SessionCommand::Halt => {
            debug::halt(&mut session)?;
            let status = debug::status(&mut session)?;
            writeln!(out, "{}", describe(state, &mut session, status)?)?;
        }
        SessionCommand::Resume => {
            debug::resume(&mut session)?;
            // The core may halt at a breakpoint again before the next poll, which would
            // otherwise look like no change.
            state.status = Some(CoreStatus::Running);
            writeln!(out, "running")?;
            if sink != Sink::Stdout {
                println!("running");
            }
        }
        SessionCommand::Step => {
            debug::step(&mut session)?;
            // A step leaves the core halted, so the status does not change; print it here.
            let status = debug::status(&mut session)?;
            state.status = Some(status);
            writeln!(out, "{}", describe(state, &mut session, status)?)?;
        }
        SessionCommand::Break { target: text } => {
            let breakpoint = state.breakpoints.add(elf, &mut session, &text)?;
            writeln!(out, "breakpoint {} at {}", breakpoint.id, breakpoint.label)?;
        }
        SessionCommand::Delete { ids } => {
            for breakpoint in state.breakpoints.delete(&mut session, &ids)? {
                writeln!(out, "breakpoint {} cleared", breakpoint.id)?;
            }
        }
        SessionCommand::Status => {
            let status = debug::status(&mut session)?;
            writeln!(out, "{}", describe(state, &mut session, status)?)?;
            for breakpoint in state.breakpoints.iter() {
                writeln!(out, "breakpoint {} at {}", breakpoint.id, breakpoint.label)?;
            }
            for watch in &state.watches {
                describe_watch(out, watch)?;
            }
        }
        SessionCommand::Quit if sink == Sink::Stdout => return Ok(Outcome::Quit),
        SessionCommand::Quit => bail!("only the terminal running the session can end it"),
    }
    Ok(Outcome::Done)
}

fn describe(state: &State, session: &mut Session, status: CoreStatus) -> Result<String> {
    debug::describe_status(state.elf.as_ref(), session, &state.breakpoints, status)
}

fn describe_watch(out: &mut String, watch: &Watch) -> Result<()> {
    writeln!(
        out,
        "watch {}: {} every {} ms",
        watch.id,
        watch.texts.join(" "),
        watch.interval.as_millis()
    )?;
    Ok(())
}

/// Print the core status when it changes. Logs written before the change, such as
/// a panic message before the HardFault it causes, print first.
fn report_status(target: &Target, state: &mut State) -> Result<()> {
    let mut session = target.session.lock();
    let status = debug::status(&mut session)?;
    if state.status != Some(status) {
        state.status = Some(status);
        if let Some(logs) = &mut state.logs {
            match logs.poll(&mut session) {
                Ok(text) => print!("{text}"),
                Err(e) => println!("error: could not read RTT: {e:#}"),
            }
        }
        println!("{}", describe(state, &mut session, status)?);
    }
    Ok(())
}

/// Print every due watch. A watch that fails reports the error and stops.
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
    // Output per watch, and the result of each watch that ended
    let mut sent = Vec::new();
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
        let text = match text {
            Ok(text) => format!("--- watch {}, {elapsed:.3} s\n{text}", watch.id),
            Err(e) => {
                sent.push((
                    watch.sink,
                    String::new(),
                    Some(Some(format!("watch {} stopped: {e:#}", watch.id))),
                ));
                return false;
            }
        };
        // Keep to the schedule; after a slow read, skip the reads it overran.
        watch.next += watch.interval;
        while watch.next <= now {
            watch.next += watch.interval;
        }
        watch.remaining = watch.remaining.map(|n| n - 1);
        if watch.remaining == Some(0) {
            sent.push((
                watch.sink,
                format!("{text}watch {} done\n", watch.id),
                Some(None),
            ));
            return false;
        }
        sent.push((watch.sink, text, None));
        true
    });
    drop(memory);
    drop(session);
    for (sink, text, ended) in sent {
        match (sink, ended) {
            (_, Some(error)) => state.finish(sink, text, error),
            (Sink::Stdout, None) => print!("{text}"),
            (Sink::Client(id), None) => state.send(
                id,
                &Response {
                    output: Some(text),
                    ..Response::default()
                },
            ),
        }
    }
}

/// Program `file`, reload its symbols and logs, and reset. Breakpoints are cleared,
/// since the functions may have moved, and watches are resolved again.
#[allow(clippy::too_many_arguments)]
fn reflash(
    target: &Target,
    session: &mut Session,
    state: &mut State,
    file: &Path,
    verify: bool,
    chip_erase: bool,
    halt: bool,
    out: &mut String,
) -> Result<()> {
    let elf =
        ElfParser::parse(file).with_context(|| format!("could not parse {}", file.display()))?;
    crate::api::forget_units();
    let logs = Logs::new(file)?;
    for breakpoint in state.breakpoints.delete(session, &[])? {
        writeln!(out, "breakpoint {} cleared", breakpoint.id)?;
    }
    flash::flash(session, file, verify, chip_erase)?;
    writeln!(out, "flashed {}", file.display())?;
    state.elf_path = Some(file.to_path_buf());
    state.elf = Some(elf);
    state.logs = logs;
    if let Some(logs) = &mut state.logs {
        logs.restart(session)?;
    }
    flash::reset(session, halt)?;
    state.status = None;
    let mut memory = target.memory(session)?;
    let elf = state.elf.as_ref();
    let mut stopped = Vec::new();
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
                stopped.push((watch.sink, format!("watch {} stopped: {e:#}", watch.id)));
                false
            }
        }
    });
    drop(memory);
    for (sink, error) in stopped {
        state.finish(sink, String::new(), Some(error));
    }
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
