use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use eframe::egui;
use tuning_studio_cli::daemon;

use crate::model::{self, Saved, Suggestion, Value};

pub enum Work {
    Connect(Saved),
    Disconnect,
    Configure(Saved),
    Polling(bool),
    List(String),
    Complete(String),
    Set { path: String, input: String },
    Control(&'static str),
    Shutdown,
}

pub enum Event {
    State {
        connected: bool,
        owned: bool,
        status: String,
    },
    Sample {
        declared: Vec<Value>,
        watched: Vec<(String, Vec<Value>)>,
        error: Option<String>,
    },
    Suggestions {
        query: String,
        items: Vec<Suggestion>,
        error: Option<String>,
    },
    Written {
        path: String,
        result: std::result::Result<String, String>,
    },
    Probes(Vec<(String, String)>),
    Notice(String),
    Log(String),
}

pub struct Backend {
    pub tx: Sender<Work>,
    pub rx: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Backend {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, work) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut worker = Worker {
                config: Saved::default(),
                connected: false,
                paused: false,
                owner: None,
                values: Vec::new(),
                events,
                ctx,
            };
            let mut next_poll = Instant::now();
            loop {
                let timeout = if worker.connected && !worker.paused {
                    next_poll.saturating_duration_since(Instant::now())
                } else {
                    Duration::from_secs(1)
                };
                match work.recv_timeout(timeout) {
                    Ok(Work::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(command) => {
                        worker.handle(command);
                        // Commands stay ahead of the next scheduled read.
                        next_poll = next_poll
                            .min(Instant::now() + Duration::from_millis(worker.config.poll_ms));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if worker.connected && !worker.paused {
                            worker.poll();
                        }
                        next_poll = Instant::now()
                            + Duration::from_millis(worker.config.poll_ms.clamp(20, 10000));
                    }
                }
            }
            worker.disconnect();
        });
        Self {
            tx,
            rx,
            thread: Some(thread),
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.tx.send(Work::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Owner {
    child: Child,
    token: String,
    probe: String,
}
struct Worker {
    config: Saved,
    connected: bool,
    paused: bool,
    owner: Option<Owner>,
    values: Vec<Value>,
    events: Sender<Event>,
    ctx: egui::Context,
}

fn request(probe: &str, args: Vec<String>) -> Result<String> {
    let socket =
        daemon::find((!probe.is_empty()).then_some(probe))?.context("No running session")?;
    exchange(socket, args)
}

fn exchange(mut socket: std::os::unix::net::UnixStream, args: Vec<String>) -> Result<String> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(&mut socket, &daemon::Request { args })?;
    socket.write_all(b"\n")?;
    let mut output = String::new();
    for line in BufReader::new(socket).take(16 * 1024 * 1024).lines() {
        let reply: daemon::Response = serde_json::from_str(&line?)?;
        if let Some(text) = reply.output {
            output.push_str(&text);
        }
        if reply.done {
            // An inspect may contain useful per-value JSON alongside an error.
            if let Some(error) = reply.error {
                if model::parse_values(&output).is_ok() {
                    return Ok(output);
                }
                bail!(error);
            }
            return Ok(output);
        }
    }
    bail!("Session ended before answering, or exceeded the response limit")
}

impl Worker {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }
    fn call(&self, args: Vec<String>) -> Result<String> {
        request(&self.config.probe, args)
    }

    fn connect(&mut self, config: Saved) -> Result<()> {
        self.disconnect();
        self.config = config;
        self.emit(Event::State {
            connected: false,
            owned: false,
            status: "Connecting…".into(),
        });
        if self.call(vec!["status".into()]).is_err() {
            ensure!(!self.config.elf.is_empty(), "Choose a firmware ELF");
            ensure!(!self.config.chip.is_empty(), "Enter the chip name");
            let token = format!(
                "desktop-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            );
            let mut command = Command::new(&self.config.cli);
            command.args([
                "attach",
                "--chip",
                &self.config.chip,
                "--speed",
                &self.config.speed.to_string(),
            ]);
            if !self.config.probe.is_empty() {
                command.args(["--probe", &self.config.probe]);
            }
            command
                .arg(&self.config.elf)
                .env("TUNING_STUDIO_OWNER_TOKEN", &token)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());
            let mut child = command
                .spawn()
                .context("Could not start the CLI; check its path")?;
            for stream in [
                child
                    .stdout
                    .take()
                    .map(|v| Box::new(v) as Box<dyn Read + Send>),
                child
                    .stderr
                    .take()
                    .map(|v| Box::new(v) as Box<dyn Read + Send>),
            ]
            .into_iter()
            .flatten()
            {
                let tx = self.events.clone();
                let ctx = self.ctx.clone();
                thread::spawn(move || {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        if tx.send(Event::Log(line)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                });
            }
            self.owner = Some(Owner {
                child,
                token,
                probe: self.config.probe.clone(),
            });
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if let Some(status) = self
                    .owner
                    .as_mut()
                    .context("Missing owned process")?
                    .child
                    .try_wait()?
                {
                    bail!("Attach exited with {status}; see Session log");
                }
                if self.call(vec!["status".into()]).is_ok() {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Attach did not become ready; see Session log"
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
        self.connected = true;
        self.poll();
        Ok(())
    }

    fn disconnect(&mut self) {
        self.connected = false;
        self.values.clear();
        if let Some(mut owner) = self.owner.take() {
            if let Err(error) = request(
                &owner.probe,
                vec!["stop".into(), "--owner".into(), owner.token],
            ) {
                self.emit(Event::Log(format!("Detach request: {error:#}")));
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while matches!(owner.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(30));
            }
            if matches!(owner.child.try_wait(), Ok(None)) {
                let _ = owner.child.kill();
                let _ = owner.child.wait();
                self.emit(Event::Log(
                    "CLI needed termination after detach timed out".into(),
                ));
            }
        }
        self.emit(Event::State {
            connected: false,
            owned: false,
            status: "Disconnected".into(),
        });
    }

    fn handle(&mut self, work: Work) {
        match work {
            Work::Connect(config) => {
                if let Err(error) = self.connect(config) {
                    self.disconnect();
                    self.emit(Event::Notice(format!("{error:#}")));
                }
            }
            Work::Disconnect => self.disconnect(),
            Work::Configure(config) => {
                if !self.connected {
                    self.config = config;
                    return;
                }
                self.config.watches = config.watches;
                self.config.expanded = config.expanded;
                self.config.locked = config.locked;
                self.config.poll_ms = config.poll_ms;
            }
            Work::Polling(paused) => self.paused = paused,
            Work::List(cli) => match Command::new(cli).arg("list").output() {
                Ok(output) if output.status.success() => {
                    let probes = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .filter_map(|line| {
                            let (selector, name) = line.trim().split_once(char::is_whitespace)?;
                            selector
                                .contains(':')
                                .then(|| (selector.to_owned(), name.trim().to_owned()))
                        })
                        .collect();
                    self.emit(Event::Probes(probes));
                }
                Ok(output) => self.emit(Event::Notice(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                )),
                Err(error) => self.emit(Event::Notice(format!("Could not list probes: {error}"))),
            },
            Work::Complete(query) => {
                let result = if self.connected {
                    self.call(vec!["complete".into(), "--".into(), query.clone()])
                } else {
                    Command::new(&self.config.cli)
                        .args(["complete", "--elf", &self.config.elf, "--", &query])
                        .output()
                        .context("Could not get suggestions")
                        .and_then(|out| {
                            ensure!(
                                out.status.success(),
                                "{}",
                                String::from_utf8_lossy(&out.stderr)
                            );
                            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
                        })
                }
                .and_then(|text| model::parse_suggestions(&text));
                let (items, error) = match result {
                    Ok(items) => (items, None),
                    Err(e) => (vec![], Some(format!("{e:#}"))),
                };
                self.emit(Event::Suggestions {
                    query,
                    items,
                    error,
                });
            }
            Work::Set { path, input } => {
                let result = (|| -> Result<String> {
                    ensure!(self.connected, "Disconnected");
                    ensure!(!self.config.locked.contains(&path), "Locked in Live Watch");
                    let value =
                        model::find(&self.values, &path).context("Value is no longer available")?;
                    ensure!(
                        value.writable && value.error.is_none(),
                        "{}",
                        value.reason()
                    );
                    self.call(vec!["set".into(), "--".into(), path.clone(), input])
                })()
                .map_err(|e| format!("{e:#}"));
                self.emit(Event::Written { path, result });
                if self.connected {
                    self.poll();
                }
            }
            Work::Control(action) => {
                if self.connected {
                    if let Err(error) = self.call(vec![action.into()]) {
                        self.emit(Event::Notice(format!("{error:#}")));
                    }
                    self.poll();
                }
            }
            Work::Shutdown => {}
        }
    }

    fn inspect(&self, location: Option<&str>) -> Result<Vec<Value>> {
        let mut args = vec!["inspect".into()];
        for path in self.config.expanded.iter().take(64) {
            args.extend(["--expand".into(), path.clone()]);
        }
        if let Some(path) = location {
            args.extend(["--".into(), path.to_owned()]);
        }
        model::parse_values(&self.call(args)?)
    }

    fn poll(&mut self) {
        let status = self.call(vec!["status".into()]);
        let Ok(status) = status else {
            self.values.clear();
            self.emit(Event::Sample {
                declared: vec![],
                watched: vec![],
                error: Some(format!(
                    "{}",
                    status
                        .err()
                        .unwrap_or_else(|| anyhow::anyhow!("Session unavailable"))
                )),
            });
            self.emit(Event::State {
                connected: true,
                owned: self.owner.is_some(),
                status: "Session unavailable".into(),
            });
            return;
        };
        self.emit(Event::State {
            connected: true,
            owned: self.owner.is_some(),
            status: status.lines().next().unwrap_or("Connected").to_owned(),
        });
        let (declared, error) = match self.inspect(None) {
            Ok(values) => (values, None),
            Err(e) => (vec![], Some(format!("{e:#}"))),
        };
        let mut watched = Vec::new();
        for path in &self.config.watches {
            let values = self.inspect(Some(path)).unwrap_or_else(|error| {
                vec![Value {
                    path: path.clone(),
                    type_name: String::new(),
                    kind: "raw".into(),
                    writable: false,
                    display: String::new(),
                    error: Some(format!("{error:#}")),
                    write_reason: None,
                    label: None,
                    children: vec![],
                    children_note: None,
                    expandable: false,
                }]
            });
            watched.push((path.clone(), values));
        }
        self.values = declared.clone();
        for (_, values) in &watched {
            self.values.extend(values.clone());
        }
        self.emit(Event::Sample {
            declared,
            watched,
            error,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn socket_exchange_collects_output_and_reports_backend_rejections() {
        fn serve(reply: &'static str) -> Result<String> {
            let (client, mut server) = UnixStream::pair()?;
            let thread = thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(server.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();
                let request: daemon::Request = serde_json::from_str(&line).unwrap();
                assert_eq!(request.args, ["status"]);
                server.write_all(reply.as_bytes()).unwrap();
            });
            let result = exchange(client, vec!["status".into()]);
            thread.join().unwrap();
            result
        }
        assert_eq!(
            serve("{\"output\":\"running\"}\n{\"done\":true}\n").unwrap(),
            "running"
        );
        assert!(
            serve("{\"done\":true,\"error\":\"read only\"}\n")
                .unwrap_err()
                .to_string()
                .contains("read only")
        );
        assert!(serve("{\"output\":\"unfinished\"}\n").is_err());
    }
}
