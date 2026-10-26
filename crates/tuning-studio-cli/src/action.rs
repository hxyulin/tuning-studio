//! The commands a running session serves. The session parses them from words, and
//! the one-shot commands in main.rs forward them as the same words, so each flag
//! and default is declared here once.

use clap::{Args, Subcommand};
use std::path::PathBuf;

use crate::flash::AfterReset;

#[derive(Subcommand, Debug, PartialEq)]
pub enum Action {
    /// Read variables or addresses once
    Read(ReadArgs),
    /// Read variables or addresses repeatedly, until `unwatch`
    Watch(WatchArgs),
    /// Sample every Embassy task, then print how often each was run-queued and
    /// where it was parked. The session pauses its other work while sampling.
    Tasks(TasksArgs),
    /// Record every write to up to four values with the DWT's data trace. The
    /// session pauses its other work while tracing.
    Trace(TraceArgs),
    /// Print every `Param` and `watch!` value
    Values,
    /// Write one value, then read it back. A `Param` changes through the
    /// firmware, which applies its range and step.
    Set(SetArgs),
    /// Halt the core
    Halt,
    /// Resume the core
    Resume,
    /// Execute one instruction on a halted core
    Step,
    /// Set a hardware breakpoint on a function or an address
    Break(BreakArgs),
    /// Clear the breakpoints with these numbers, or all of them
    Delete(DeleteArgs),
    /// Reset the core and let it run
    Reset(ResetArgs),
    /// Program an ELF, by default the session's, which may have been rebuilt; then
    /// reset. The session uses the new ELF's symbols and logs from then on.
    Flash {
        elf: Option<PathBuf>,
        #[command(flatten)]
        options: FlashArgs,
    },
    /// Print the core status, breakpoints and watches
    Status,
}

#[derive(Args, Debug, PartialEq)]
pub struct ReadArgs {
    /// Symbol paths, such as `CONTROL.yaw.target_rpm` or `buffer[3]`, or addresses
    /// with an optional type and path, such as `0x24000070`, `0x24000070:f32` or
    /// `0x20000038:Axis.gains.2`. Types other than the primitives need an ELF.
    #[arg(required = true)]
    pub locations: Vec<String>,
}

#[derive(Args, Debug, PartialEq)]
pub struct WatchArgs {
    /// Symbol paths or addresses, as for `read`
    #[arg(required = true)]
    pub locations: Vec<String>,
    /// Time between reads in milliseconds; 0 reads as fast as the probe can.
    /// A session reads at most every 1 ms.
    #[arg(long, default_value_t = 100)]
    pub interval: u64,
    /// Stop after this many reads
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub count: Option<u64>,
}

#[derive(Args, Debug, PartialEq)]
pub struct TasksArgs {
    /// How long to sample, in milliseconds
    #[arg(long, default_value_t = 1000)]
    pub duration: u64,
}

#[derive(Args, Debug, PartialEq)]
pub struct TraceArgs {
    /// Symbol paths or addresses of values of at most 4 bytes, or `watch!`
    /// values and `Param`s holding one
    #[arg(required = true)]
    pub locations: Vec<String>,
    /// How long to trace, in milliseconds
    #[arg(long, default_value_t = 1000)]
    pub duration: u64,
}

#[derive(Args, Debug, PartialEq)]
pub struct SetArgs {
    /// Symbol path or address, as for `read`
    pub location: String,
    #[arg(allow_negative_numbers = true)]
    pub value: String,
}

#[derive(Args, Debug, PartialEq)]
pub struct BreakArgs {
    /// Function name, such as `SysTick` or `control::tick`, or an address
    pub target: String,
}

#[derive(Args, Debug, PartialEq)]
pub struct DeleteArgs {
    pub ids: Vec<u32>,
}

#[derive(Args, Debug, PartialEq)]
pub struct ResetArgs {
    /// Halt at the reset vector instead of running
    #[arg(long)]
    pub halt: bool,
}

#[derive(Args, Debug, PartialEq)]
pub struct FlashArgs {
    /// Read the flash back after programming
    #[arg(long)]
    pub verify: bool,
    /// Erase the whole chip rather than only the sectors the ELF uses
    #[arg(long)]
    pub chip_erase: bool,
    /// Halt at the reset vector instead of running
    #[arg(long)]
    pub halt: bool,
}

fn push(words: &mut Vec<String>, parts: &[&str]) {
    words.extend(parts.iter().map(|part| part.to_string()));
}

impl ResetArgs {
    pub fn after_reset(&self) -> AfterReset {
        AfterReset::from_halt(self.halt)
    }
}

impl FlashArgs {
    pub fn after_reset(&self) -> AfterReset {
        AfterReset::from_halt(self.halt)
    }
}

impl Action {
    /// The words the session parses back into this action
    pub fn to_words(&self) -> Vec<String> {
        let mut words = Vec::new();
        match self {
            Action::Read(args) => {
                push(&mut words, &["read"]);
                words.extend(args.locations.iter().cloned());
            }
            Action::Watch(args) => {
                push(&mut words, &["watch"]);
                words.extend(args.locations.iter().cloned());
                push(&mut words, &["--interval", &args.interval.to_string()]);
                if let Some(count) = args.count {
                    push(&mut words, &["--count", &count.to_string()]);
                }
            }
            Action::Tasks(args) => push(
                &mut words,
                &["tasks", "--duration", &args.duration.to_string()],
            ),
            Action::Trace(args) => {
                push(&mut words, &["trace"]);
                words.extend(args.locations.iter().cloned());
                push(&mut words, &["--duration", &args.duration.to_string()]);
            }
            Action::Values => push(&mut words, &["values"]),
            Action::Set(args) => push(&mut words, &["set", &args.location, &args.value]),
            Action::Halt => push(&mut words, &["halt"]),
            Action::Resume => push(&mut words, &["resume"]),
            Action::Step => push(&mut words, &["step"]),
            Action::Break(args) => push(&mut words, &["break", &args.target]),
            Action::Delete(args) => {
                push(&mut words, &["delete"]);
                words.extend(args.ids.iter().map(u32::to_string));
            }
            Action::Reset(args) => {
                push(&mut words, &["reset"]);
                if args.halt {
                    push(&mut words, &["--halt"]);
                }
            }
            Action::Flash { elf, options } => {
                push(&mut words, &["flash"]);
                if let Some(elf) = elf {
                    push(&mut words, &[&elf.to_string_lossy()]);
                }
                for (set, flag) in [
                    (options.verify, "--verify"),
                    (options.chip_erase, "--chip-erase"),
                    (options.halt, "--halt"),
                ] {
                    if set {
                        push(&mut words, &[flag]);
                    }
                }
            }
            Action::Status => push(&mut words, &["status"]),
        }
        words
    }
}
