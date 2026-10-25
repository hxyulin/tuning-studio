//! Programming, erasing and resetting through probe-rs, with one progress line per
//! phase.

use anyhow::{Context, Result};
use probe_rs::Session;
use probe_rs::flashing::{
    self, DownloadOptions, ElfLoader, ElfOptions, FlashProgress, ProgressEvent, ProgressOperation,
};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// Program the ELF's loadable sections into flash.
pub fn flash(session: &mut Session, elf: &Path, verify: bool, chip_erase: bool) -> Result<()> {
    let mut progress = Progress::default();
    let mut options = DownloadOptions::default();
    options.verify = verify;
    options.do_chip_erase = chip_erase;
    options.progress = FlashProgress::new(|event| progress.event(event));
    flashing::download_file_with_options(session, elf, ElfLoader(ElfOptions::default()), options)
        .with_context(|| format!("could not flash {}", elf.display()))
}

/// Erase all flash.
pub fn erase(session: &mut Session) -> Result<()> {
    let start = Instant::now();
    flashing::erase_all(session, &mut FlashProgress::empty(), false)
        .context("could not erase the flash")?;
    println!("erased the flash in {:.2} s", start.elapsed().as_secs_f64());
    Ok(())
}

/// Reset core 0 and let it run, or halt it at the reset vector.
pub fn reset(session: &mut Session, halt: bool) -> Result<()> {
    let mut core = session.core(0)?;
    if halt {
        core.reset_and_halt(Duration::from_millis(500))?;
    } else {
        core.reset()?;
    }
    Ok(())
}

#[derive(Default)]
struct Progress {
    operation: Option<(ProgressOperation, Instant)>,
    done: u64,
}

impl Progress {
    fn event(&mut self, event: ProgressEvent) {
        let terminal = std::io::stdout().is_terminal();
        match event {
            ProgressEvent::AddProgressBar { .. } | ProgressEvent::FlashLayoutReady { .. } => {}
            ProgressEvent::Started(operation) => {
                self.operation = Some((operation, Instant::now()));
                self.done = 0;
            }
            ProgressEvent::Progress { size, .. } => {
                self.done += size;
                if terminal && let Some((operation, _)) = self.operation {
                    print!("\r{} {} KiB", verb(operation), self.done / 1024);
                    let _ = std::io::stdout().flush();
                }
            }
            ProgressEvent::Finished(operation) | ProgressEvent::Failed(operation) => {
                let failed = matches!(event, ProgressEvent::Failed(_));
                let elapsed = self
                    .operation
                    .take()
                    .map_or(0.0, |(_, start)| start.elapsed().as_secs_f64());
                if terminal {
                    print!("\r\x1b[K");
                }
                if failed {
                    println!("{} failed after {} KiB", verb(operation), self.done / 1024);
                } else if self.done > 0 {
                    println!(
                        "{} {} KiB in {elapsed:.2} s",
                        past(operation),
                        self.done / 1024
                    );
                }
            }
            ProgressEvent::DiagnosticMessage { message } => println!("flash algorithm: {message}"),
        }
    }
}

fn verb(operation: ProgressOperation) -> &'static str {
    match operation {
        ProgressOperation::Fill => "reading",
        ProgressOperation::Erase => "erasing",
        ProgressOperation::Program => "programming",
        ProgressOperation::Verify => "verifying",
    }
}

fn past(operation: ProgressOperation) -> &'static str {
    match operation {
        ProgressOperation::Fill => "read",
        ProgressOperation::Erase => "erased",
        ProgressOperation::Program => "programmed",
        ProgressOperation::Verify => "verified",
    }
}
