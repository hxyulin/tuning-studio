//! RTT logs: defmt frames on a channel named `defmt`, text on every other up
//! channel. Reading RTT goes through the MEM-AP, so it never halts the core.
//!
//! The control block address comes from the ELF. After a reset the firmware writes
//! a new control block, so `restart` clears the old one and `poll` attaches again
//! once the firmware has started.

use anyhow::{Context, Result};
use defmt_decoder::log::format::{Formatter, FormatterConfig, FormatterFormat};
use defmt_decoder::{DecodeError, Locations, StreamDecoder, Table};
use probe_rs::Session;
use probe_rs::rtt::{Rtt, ScanRegion};
use std::path::Path;

pub struct Logs {
    control_block: u64,
    defmt: Option<Defmt>,
    rtt: Option<Rtt>,
    channels: Vec<Channel>,
    buffer: Vec<u8>,
}

struct Defmt {
    table: &'static Table,
    locations: Option<Locations>,
    formatter: Formatter,
}

struct Channel {
    decoder: Option<Box<dyn StreamDecoder + Send + Sync + 'static>>,
    /// Text up to the last newline not printed yet
    partial: Vec<u8>,
}

impl Logs {
    /// Logs for the firmware in `elf`, or `None` when it has no RTT control block.
    pub fn new(elf: &Path) -> Result<Option<Self>> {
        let bytes =
            std::fs::read(elf).with_context(|| format!("could not read {}", elf.display()))?;
        let Some(control_block) = probe_rs::rtt::find_rtt_control_block_in_raw_file(&bytes)
            .context("could not parse the ELF")?
        else {
            return Ok(None);
        };
        let defmt = match Table::parse(&bytes).context("could not read the defmt table")? {
            // The decoders borrow the table for the whole run.
            Some(table) => {
                let table: &'static Table = Box::leak(Box::new(table));
                let locations = table.get_locations(&bytes).ok().filter(|locations| {
                    table.indices().all(|i| locations.contains_key(&(i as u64)))
                });
                let mut config = FormatterConfig {
                    format: FormatterFormat::OneLine {
                        with_location: locations.is_some(),
                    },
                    is_timestamp_available: false,
                };
                if table.has_timestamp() {
                    config = config.with_timestamp();
                }
                Some(Defmt {
                    table,
                    locations,
                    formatter: Formatter::new(config),
                })
            }
            None => None,
        };
        Ok(Some(Self {
            control_block,
            defmt,
            rtt: None,
            channels: Vec::new(),
            buffer: vec![0; 4096],
        }))
    }

    /// Forget the control block before a reset, so `poll` waits for the firmware to
    /// write a new one rather than reading the old buffers.
    pub fn restart(&mut self, session: &mut Session) -> Result<()> {
        self.rtt = None;
        self.channels.clear();
        Rtt::clear_control_block(
            &mut session.core(0)?,
            &ScanRegion::Exact(self.control_block),
        )?;
        Ok(())
    }

    /// Read every up channel and return the complete lines received.
    pub fn poll(&mut self, session: &mut Session) -> Result<String> {
        let mut core = session.core(0)?;
        if self.rtt.is_none() {
            // Until the firmware has set up the control block, there is nothing to read.
            let Ok(mut rtt) = Rtt::attach_at(&mut core, self.control_block) else {
                return Ok(String::new());
            };
            self.channels = rtt
                .up_channels()
                .iter()
                .map(|up| Channel {
                    decoder: (up.name() == Some("defmt"))
                        .then_some(self.defmt.as_ref())
                        .flatten()
                        .map(|defmt| defmt.table.new_stream_decoder()),
                    partial: Vec::new(),
                })
                .collect();
            self.rtt = Some(rtt);
        }
        let rtt = self.rtt.as_mut().unwrap();
        let mut text = String::new();
        for (up, channel) in rtt.up_channels().iter_mut().zip(&mut self.channels) {
            loop {
                let n = up.read(&mut core, &mut self.buffer)?;
                if n == 0 {
                    break;
                }
                let data = &self.buffer[..n];
                match &mut channel.decoder {
                    Some(decoder) => {
                        decoder.received(data);
                        decode(self.defmt.as_ref().unwrap(), decoder.as_mut(), &mut text);
                    }
                    None => {
                        channel.partial.extend_from_slice(data);
                        if let Some(end) = channel.partial.iter().rposition(|&b| b == b'\n') {
                            let line: Vec<u8> = channel.partial.drain(..=end).collect();
                            text += &String::from_utf8_lossy(&line);
                        }
                    }
                }
            }
        }
        Ok(text)
    }
}

fn decode(defmt: &Defmt, decoder: &mut dyn StreamDecoder, text: &mut String) {
    loop {
        match decoder.decode() {
            Ok(frame) => {
                let location = defmt
                    .locations
                    .as_ref()
                    .and_then(|locations| locations.get(&frame.index()));
                let file = location.map(|l| l.file.display().to_string());
                let line = location.map(|l| l.line as u32);
                let module = location.map(|l| l.module.as_str());
                *text += &defmt
                    .formatter
                    .format_frame(frame, file.as_deref(), line, module);
                text.push('\n');
            }
            Err(DecodeError::UnexpectedEof) => break,
            Err(DecodeError::Malformed) if defmt.table.encoding().can_recover() => {}
            Err(DecodeError::Malformed) => {
                *text += "error: a defmt frame was malformed; later frames may be lost\n";
                break;
            }
        }
    }
}
