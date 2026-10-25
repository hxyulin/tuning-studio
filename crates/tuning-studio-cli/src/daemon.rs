//! The local socket through which other commands reach a running `run` or `attach`.
//!
//! Each session listens on a Unix socket named after its probe, in a directory only
//! this user can open. A client sends one JSON request per line and reads JSON
//! responses until one has `done` set:
//!
//! ```text
//! > {"args":["read","live::TICKS"]}
//! < {"output":"type_lab::live::TICKS: Atomic<u32> = 2987297\n"}
//! < {"done":true}
//! ```
//!
//! `args` uses the session grammar that `help` lists. A `watch` sends output until
//! it ends, so a watch without `--count` never sends `done`; closing the
//! connection stops it.

use anyhow::{Context, Result, bail};
use probe_rs::probe::DebugProbeSelector;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Response {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The probe part of a socket name, such as `ef1a-74e5-7844331832`.
pub fn probe_id(vendor_id: u16, product_id: u16, serial: Option<&str>) -> String {
    match serial {
        Some(serial) => format!("{vendor_id:04x}-{product_id:04x}-{serial}"),
        None => format!("{vendor_id:04x}-{product_id:04x}"),
    }
}

/// `$XDG_RUNTIME_DIR/tuning-studio`, or a directory for this user in the temp dir.
fn dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime) => PathBuf::from(runtime).join("tuning-studio"),
        // SAFETY: getuid has no preconditions.
        None => std::env::temp_dir().join(format!("tuning-studio-{}", unsafe { libc::getuid() })),
    }
}

/// A bound socket, removed again on drop.
pub struct Listener {
    pub listener: UnixListener,
    path: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn listen(probe_id: &str) -> Result<Listener> {
    let dir = dir();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .with_context(|| format!("could not create {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = dir.join(format!("{probe_id}.sock"));
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            bail!(
                "another session already serves this probe at {}",
                path.display()
            );
        }
        // Left behind by a session that did not exit cleanly.
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("could not listen on {}", path.display()))?;
    Ok(Listener { listener, path })
}

/// Connect to the session for `probe`, or for the only session running when
/// `probe` is `None`. Returns `None` when no session serves it.
pub fn find(probe: Option<&str>) -> Result<Option<UnixStream>> {
    let selector = probe.map(str::parse::<DebugProbeSelector>).transpose()?;
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return Ok(None);
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".sock"))
        else {
            continue;
        };
        if let Some(selector) = &selector
            && !selects(selector, name)
        {
            continue;
        }
        match UnixStream::connect(&path) {
            Ok(stream) => found.push((name.to_string(), stream)),
            Err(_) => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop().map(|(_, stream)| stream)),
        _ => {
            let names: Vec<_> = found
                .iter()
                .map(|(name, _)| name.replace('-', ":"))
                .collect();
            bail!(
                "sessions are running for several probes ({}); pick one with --probe",
                names.join(", ")
            )
        }
    }
}

fn selects(selector: &DebugProbeSelector, name: &str) -> bool {
    let mut parts = name.splitn(3, '-');
    let vendor = parts.next().and_then(|v| u16::from_str_radix(v, 16).ok());
    let product = parts.next().and_then(|p| u16::from_str_radix(p, 16).ok());
    let serial = parts.next();
    vendor == Some(selector.vendor_id)
        && product == Some(selector.product_id)
        && selector
            .serial_number
            .as_deref()
            .is_none_or(|wanted| serial == Some(wanted))
}

/// Send one request and print its output until it is done.
pub fn request(stream: UnixStream, args: Vec<String>) -> Result<()> {
    let mut writer = stream.try_clone()?;
    serde_json::to_writer(&mut writer, &Request { args })?;
    writer.write_all(b"\n")?;
    let mut stdout = std::io::stdout();
    for line in BufReader::new(stream).lines() {
        let response: Response =
            serde_json::from_str(&line?).context("the session sent a bad response")?;
        if let Some(output) = response.output {
            stdout.write_all(output.as_bytes())?;
            stdout.flush()?;
        }
        if response.done {
            return match response.error {
                Some(error) => bail!("{error}"),
                None => Ok(()),
            };
        }
    }
    bail!("the session ended before answering")
}

/// Make a path absolute, since the session may run in another directory.
pub fn absolute(path: &Path) -> Result<String> {
    Ok(std::path::absolute(path)?.to_string_lossy().into_owned())
}
