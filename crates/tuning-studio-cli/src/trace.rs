//! Every write to up to four values, from the DWT's data trace, while the
//! firmware runs.
//!
//! Each DWT comparator matches one address. On every core access to it, the DWT
//! sends the access's PC and value to the ITM, which adds a timestamp in core
//! cycles. The trace funnel routes the ITM's packets into the ETF, 2 KB of
//! on-chip trace memory on the STM32H723, which the probe drains through the
//! debug port. Nothing halts the core. When the ETF fills, the ITM drops
//! packets and reports an overflow.
//!
//! probe-rs drains the ETF one word per probe round trip, so this module drains
//! it itself: the MEM-AP's banked data register reads the ETF's read-data
//! register without advancing the address, so one block transfer empties the
//! ETF.

use anyhow::{Context, Result, anyhow, bail};
use probe_rs::architecture::arm::FullyQualifiedApAddress;
use probe_rs::architecture::arm::component::{Dwt, Itm, TraceFunnel, find_component};
use probe_rs::architecture::arm::dp::DpAddress;
use probe_rs::architecture::arm::memory::{CoresightComponent, PeripheralType};
use probe_rs::{MemoryInterface, Session};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use studio_dwarf::{ElfInfo, SymbolNode, VariableType};

use crate::api;
use crate::memory::Memory;
use crate::path::{self, Location};
use crate::target::Target;
use crate::tasks::Functions;
use crate::value;

/// Debug Exception and Monitor Control Register; bit 24 (TRCENA) enables the
/// DWT and ITM
const DEMCR: u64 = 0xE000_EDFC;
const TRCENA: u32 = 1 << 24;
/// DWT cycle counter, which the ITM's timestamps count in
const DWT_CYCCNT: u64 = 0xE000_1004;
const CYCCNT_INTERVAL: Duration = Duration::from_millis(100);
/// ITM Trace Control Register, as an offset; GTSFREQ is in bits 11:10
const ITM_TCR: u32 = 0xE80;
const GTSFREQ: u32 = 0b11 << 10;
/// The funnel input the ITM is on, as probe-rs routes it on the STM32H7
const FUNNEL_ITM_PORT: u8 = 0b10;

/// ETF registers, as offsets from its base
const ETF_STS: u32 = 0x0C;
const STS_READY: u32 = 1 << 2;
const ETF_RRD: u64 = 0x10;
const ETF_CTL: u32 = 0x20;
const ETF_MODE: u32 = 0x28;
const MODE_SOFTWARE_FIFO: u32 = 1;
const ETF_CBUFLVL: u64 = 0x30;
/// The trace source ID probe-rs's `Itm::tx_enable` gives the ITM
const ITM_ATID: u8 = 13;
/// Formatter control. In its FIFO modes the ETF formats its input into 16-byte
/// frames whatever this register says.
const ETF_FFCR: u64 = 0x304;
const FLUSH_MAN: u32 = 1 << 6;
/// MEM-AP registers: the transfer address, and the first banked data register,
/// which accesses the address's 16-byte block plus 0 without changing it
const AP_TAR: u64 = 0x04;
const AP_BD0: u64 = 0x10;

/// A value to trace: one DWT comparator, on one address
pub struct Traced {
    label: String,
    address: u64,
    scalar: VariableType,
    unit: String,
}

/// The value at `location`, if the DWT can match it: a scalar of at most 4
/// bytes, or a `watch!` value or `Param` holding one.
pub fn traced(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    location: &Location,
) -> Result<Traced> {
    let (label, node) = match location {
        &Location::Raw { address, scalar } => {
            return check(Traced {
                label: format!("{address:#010x}"),
                address,
                scalar,
                unit: String::new(),
            });
        }
        Location::Node(node) => (node.path.clone(), node),
    };
    let elf = elf.ok_or_else(|| anyhow!("symbol paths need an ELF"))?;
    let (value, unit): (SymbolNode, String) = match api::kind(node) {
        Some(_) => api::traced_value(elf, memory, node)?,
        None => ((**node).clone(), String::new()),
    };
    let scalar = value::scalar_of(&value).ok_or_else(|| {
        anyhow!(
            "{label} is a {}, not a single value; trace one of its fields",
            value.type_name
        )
    })?;
    check(Traced {
        label,
        address: value.address,
        scalar,
        unit,
    })
}

fn check(traced: Traced) -> Result<Traced> {
    let size = value::size_of(traced.scalar) as u64;
    if size > 4 {
        bail!(
            "{} is {size} bytes; the DWT traces values of at most 4",
            traced.label
        );
    }
    if !traced.address.is_multiple_of(size) {
        bail!("{} is not aligned to its size", traced.label);
    }
    Ok(traced)
}

/// Trace the values at `locations`, which are text as `read` takes it.
pub fn run(
    target: &Target,
    session: &mut Session,
    elf: Option<&ElfInfo>,
    locations: &[String],
    duration: Duration,
    stop: &AtomicBool,
) -> Result<String> {
    let values = {
        let mut memory = target.memory(session)?;
        locations
            .iter()
            .map(|text| {
                let location = path::locate(elf, &mut memory, text)?;
                traced(elf, &mut memory, &location)
            })
            .collect::<Result<Vec<_>>>()?
    };
    trace(session, elf, &values, duration, stop)
}

/// Trace every write to `values` for `duration`, then print each with its time,
/// and a summary per value.
pub fn trace(
    session: &mut Session,
    elf: Option<&ElfInfo>,
    values: &[Traced],
    duration: Duration,
    stop: &AtomicBool,
) -> Result<String> {
    let mut etf = Etf::start(session, values)?;
    let result = etf.collect(session, duration, stop);
    let stopped = etf.stop(session);
    let capture = result?;
    stopped?;
    Ok(report(elf, values, &capture))
}

/// What a trace collected
struct Capture {
    /// Packets, by the core cycle of their timestamp
    packets: Vec<(u64, Packet)>,
    /// How long collection took
    elapsed: Duration,
    /// The core cycles that passed in that time
    cycles: u64,
}

struct Etf {
    ap: FullyQualifiedApAddress,
    base: u64,
    /// DEMCR before tracing, restored after
    demcr: u32,
    dwt: CoresightComponent,
    /// The comparators in use
    units: usize,
}

impl Etf {
    /// Route the DWT's data trace through the ITM and the trace funnel into the
    /// ETF. probe-rs's `setup_tracing` also sets up the funnel to SWO, which the
    /// STM32H723 does not have.
    fn start(session: &mut Session, values: &[Traced]) -> Result<Self> {
        let components = session.get_arm_components(DpAddress::Default)?;
        let find = |kind, what| {
            find_component(&components, kind).with_context(|| format!("the target has no {what}"))
        };
        let (dwt, itm) = (
            find(PeripheralType::Dwt, "DWT")?,
            find(PeripheralType::Itm, "ITM")?,
        );
        let funnel = find(PeripheralType::TraceFunnel, "trace funnel")?;
        let etf = find(PeripheralType::Tmc, "ETF to collect trace in")?;

        let demcr = {
            let mut core = session.core(0)?;
            let demcr = core.read_word_32(DEMCR)?;
            core.write_word_32(DEMCR, demcr | TRCENA)?;
            demcr
        };
        let mut started = Self {
            ap: etf.ap_address.clone(),
            base: etf.component.id().component_address(),
            demcr,
            dwt: dwt.clone(),
            // Counted up as comparators are set, so an undo touches only those.
            units: 0,
        };
        // From here the hardware is changed, so a failure undoes it. The setup's
        // error is the one worth reporting, not the undo's.
        if let Err(e) = started.configure(session, values, (dwt, itm, funnel, etf)) {
            let _ = started.stop(session);
            return Err(e);
        }
        Ok(started)
    }

    /// Enable the trace units and the comparators, after TRCENA is set.
    fn configure(
        &mut self,
        session: &mut Session,
        values: &[Traced],
        (dwt, itm, funnel, etf): (
            &CoresightComponent,
            &CoresightComponent,
            &CoresightComponent,
            &CoresightComponent,
        ),
    ) -> Result<()> {
        let comparators = dwt.read_reg(session.get_arm_interface()?, 0)? >> 28;
        if values.len() > comparators as usize {
            bail!(
                "the DWT has {comparators} comparators, so it traces at most {comparators} values"
            );
        }
        let interface = session.get_arm_interface()?;
        // The cycle counter, which the timestamps count, and sync packets
        Dwt::new(interface, dwt)
            .enable()
            .context("could not enable the DWT")?;
        // Only the data trace: exception trace and global timestamps would fill
        // the ETF faster than the probe drains it.
        Dwt::new(interface, dwt)
            .disable_exception_trace()
            .context("could not disable the DWT's exception trace")?;
        let mut itm_unit = Itm::new(interface, itm);
        itm_unit.unlock().context("could not unlock the ITM")?;
        itm_unit.tx_enable().context("could not enable the ITM")?;
        let tcr = itm
            .read_reg(interface, ITM_TCR)
            .context("could not read the ITM's control register")?;
        itm.write_reg(interface, ITM_TCR, tcr & !GTSFREQ)
            .context("could not write the ITM's control register")?;
        let mut funnel = TraceFunnel::new(interface, funnel);
        funnel
            .unlock()
            .context("could not unlock the trace funnel")?;
        funnel
            .enable_port(FUNNEL_ITM_PORT)
            .context("could not open the trace funnel's ITM port")?;
        // Restart the ETF empty, as a FIFO the probe reads.
        etf.write_reg(interface, ETF_CTL, 0)
            .context("could not disable the ETF")?;
        let start = Instant::now();
        while etf
            .read_reg(interface, ETF_STS)
            .context("could not read the ETF's status")?
            & STS_READY
            == 0
        {
            if start.elapsed() > Duration::from_millis(100) {
                bail!("the ETF did not stop");
            }
        }
        etf.write_reg(interface, ETF_MODE, MODE_SOFTWARE_FIFO)
            .context("could not set the ETF to FIFO mode")?;
        etf.write_reg(interface, ETF_CTL, 1)
            .context("could not enable the ETF")?;
        for (unit, traced) in values.iter().enumerate() {
            let address = u32::try_from(traced.address)
                .with_context(|| format!("{} is not a 32-bit address", traced.label))?;
            self.units = unit + 1;
            Dwt::new(interface, dwt)
                .enable_data_trace(unit, address)
                .with_context(|| format!("could not set up DWT comparator {unit}"))?;
        }
        Ok(())
    }

    /// Drain the ETF until `duration` passes or `stop` is set.
    fn collect(
        &mut self,
        session: &mut Session,
        duration: Duration,
        stop: &AtomicBool,
    ) -> Result<Capture> {
        let mut frames = Frames::default();
        let mut decoder = Decoder::default();
        let mut packets = Vec::new();
        let first = session.core(0)?.read_word_32(DWT_CYCCNT)?;
        let start = Instant::now();
        let mut cycles = 0u64;
        let mut last = first;
        let mut counted = start;
        // CYCCNT wraps every few seconds; reading it this often counts the wraps.
        let mut count = |session: &mut Session| -> Result<()> {
            let now = session.core(0)?.read_word_32(DWT_CYCCNT)?;
            cycles += u64::from(now.wrapping_sub(last));
            last = now;
            Ok(())
        };
        while start.elapsed() < duration && !stop.load(Ordering::Relaxed) {
            decoder.feed(&frames.feed(&self.drain(session)?), &mut packets);
            if counted.elapsed() > CYCCNT_INTERVAL {
                count(session)?;
                counted = Instant::now();
            }
        }
        let elapsed = start.elapsed();
        count(session)?;
        // Stop the source first: the flush waits for the ETF to take what the ITM
        // holds, which it cannot while it is full.
        self.stop_comparators(session)?;
        self.flush(session, &mut |bytes| {
            decoder.feed(&frames.feed(&bytes), &mut packets)
        })?;
        Ok(Capture {
            packets,
            elapsed,
            cycles,
        })
    }

    /// Every word the ETF holds
    fn drain(&self, session: &mut Session) -> Result<Vec<u8>> {
        let interface = session.get_arm_interface()?;
        let level = {
            let mut memory = interface.memory_interface(&self.ap)?;
            memory.read_word_32(self.base + ETF_CBUFLVL)?
        };
        if level == 0 {
            return Ok(Vec::new());
        }
        let mut data = vec![0u32; level as usize];
        interface.write_raw_ap_register(&self.ap, AP_TAR, (self.base + ETF_RRD) as u32)?;
        interface.read_raw_ap_register_repeated(&self.ap, AP_BD0, &mut data)?;
        Ok(data.iter().flat_map(|w| w.to_le_bytes()).collect())
    }

    /// Push a partial frame out of the formatter, so the last writes arrive,
    /// and drain the ETF into `feed` until the flush completes.
    fn flush(&self, session: &mut Session, feed: &mut dyn FnMut(Vec<u8>)) -> Result<()> {
        let ffcr = |session: &mut Session| -> Result<u32> {
            let interface = session.get_arm_interface()?;
            let mut memory = interface.memory_interface(&self.ap)?;
            Ok(memory.read_word_32(self.base + ETF_FFCR)?)
        };
        let value = ffcr(session)?;
        session
            .get_arm_interface()?
            .memory_interface(&self.ap)?
            .write_word_32(self.base + ETF_FFCR, value | FLUSH_MAN)?;
        let start = Instant::now();
        loop {
            feed(self.drain(session)?);
            if ffcr(session)? & FLUSH_MAN == 0 {
                feed(self.drain(session)?);
                return Ok(());
            }
            if start.elapsed() > Duration::from_secs(1) {
                bail!("the ETF did not finish flushing");
            }
        }
    }

    fn stop_comparators(&mut self, session: &mut Session) -> Result<()> {
        let interface = session.get_arm_interface()?;
        for unit in 0..self.units {
            Dwt::new(interface, &self.dwt).disable_data_trace(unit)?;
        }
        Ok(())
    }

    fn stop(&mut self, session: &mut Session) -> Result<()> {
        self.stop_comparators(session)?;
        let interface = session.get_arm_interface()?;
        interface
            .memory_interface(&self.ap)?
            .write_word_32(self.base + u64::from(ETF_CTL), 0)?;
        session.core(0)?.write_word_32(DEMCR, self.demcr)?;
        Ok(())
    }
}

/// The ITM's bytes from the ETF's 16-byte frames, across drains. Even bytes
/// hold either data, whose low bit is in byte 15, or a new source ID; odd bytes
/// always hold data.
#[derive(Default)]
struct Frames {
    pending: Vec<u8>,
    /// The source of the data bytes
    id: u8,
}

impl Frames {
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        let (frames, _) = self.pending.as_chunks::<16>();
        let whole = frames.len() * 16;
        for frame in frames {
            let mut push = |id: u8, byte: u8| {
                if id == ITM_ATID {
                    out.push(byte);
                }
            };
            let mut i = 0;
            while i < 15 {
                let byte = frame[i];
                let low = (frame[15] >> (i / 2)) & 1;
                if i % 2 == 1 {
                    push(self.id, byte);
                } else if byte & 1 == 0 {
                    push(self.id, byte | low);
                } else {
                    // A new ID. Its low bit in byte 15 says whether the next byte
                    // still belongs to the old one.
                    let new = byte >> 1;
                    if i < 14 {
                        push(if low == 1 { self.id } else { new }, frame[i + 1]);
                    }
                    self.id = new;
                    i += 1;
                }
                i += 1;
            }
        }
        self.pending.drain(..whole);
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Packet {
    /// The PC of an access a comparator matched
    Pc { unit: u8, pc: u32 },
    /// The value of an access a comparator matched
    Data { unit: u8, write: bool, value: u32 },
    /// The ITM dropped packets because the trace path was full.
    Overflow,
}

/// ITM packets from a byte stream, across drains
#[derive(Default)]
struct Decoder {
    pending: Vec<u8>,
    /// Core cycles at the last local timestamp
    cycles: u64,
    /// Packets since the last local timestamp, which applies to them
    untimed: Vec<Packet>,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<(u64, Packet)>) {
        self.pending.extend_from_slice(bytes);
        let mut i = 0;
        while let Some(len) = self.packet(i, out) {
            i += len;
        }
        self.pending.drain(..i);
    }

    /// Decode the packet at `i`, and return its length, or `None` if it is not
    /// all there yet.
    fn packet(&mut self, i: usize, out: &mut Vec<(u64, Packet)>) -> Option<usize> {
        let bytes = &self.pending[i..];
        let &header = bytes.first()?;
        // The length of a packet whose payload bytes have a continuation bit,
        // up to `max` of them. A packet that is still continued after `max`
        // ends there, so a malformed one cannot stall the decoder.
        let continued = |max: usize| {
            let end = (1..bytes.len().min(1 + max))
                .find(|&j| bytes[j] & 0x80 == 0)
                .map(|j| j + 1);
            end.or((bytes.len() > max).then_some(1 + max))
        };
        Some(match header {
            // Synchronization: zeros, then 0x80
            0x00 | 0x80 => 1,
            0x70 => {
                self.untimed.push(Packet::Overflow);
                1
            }
            // Global timestamps: GTS1 holds up to 4 bytes, GTS2 up to 6.
            0x94 => continued(4)?,
            0xB4 => continued(6)?,
            // Local timestamp, format 1: the delta follows in continuation bytes.
            h if h & 0xCF == 0xC0 => {
                let len = continued(4)?;
                let delta = bytes[1..len]
                    .iter()
                    .enumerate()
                    .fold(0u64, |acc, (k, b)| acc | (u64::from(b & 0x7F) << (7 * k)));
                self.stamp(delta, out);
                len
            }
            // Local timestamp, format 2: a delta of 1 to 6 in the header
            h if h & 0x8F == 0 => {
                self.stamp(u64::from(h >> 4), out);
                1
            }
            // Extension
            h if h & 0x0B == 0x08 => {
                if h & 0x80 != 0 {
                    continued(4)?
                } else {
                    1
                }
            }
            h if h & 0x03 != 0 => {
                let size = [0, 1, 2, 4][usize::from(h & 3)];
                if bytes.len() < 1 + size {
                    return None;
                }
                let mut value = [0u8; 4];
                value[..size].copy_from_slice(&bytes[1..=size]);
                let value = u32::from_le_bytes(value);
                // Hardware source packets have bit 2 set; the ID is in bits 7:3.
                let id = h >> 3;
                if h & 0x04 != 0 {
                    match id {
                        8..=15 if id & 1 == 0 => self.untimed.push(Packet::Pc {
                            unit: (id >> 1) & 3,
                            pc: value,
                        }),
                        16..=23 => self.untimed.push(Packet::Data {
                            unit: (id >> 1) & 3,
                            write: id & 1 == 1,
                            value,
                        }),
                        _ => {}
                    }
                }
                1 + size
            }
            _ => 1,
        })
    }

    fn stamp(&mut self, delta: u64, out: &mut Vec<(u64, Packet)>) {
        self.cycles += delta;
        out.extend(self.untimed.drain(..).map(|p| (self.cycles, p)));
    }
}

/// Each write with its time, then per value how many writes and reads there
/// were and which functions wrote it.
fn report(elf: Option<&ElfInfo>, values: &[Traced], capture: &Capture) -> String {
    let Capture {
        packets,
        elapsed,
        cycles,
    } = capture;
    let functions = elf.map(Functions::new);
    let hz = *cycles as f64 / elapsed.as_secs_f64();
    let mut out = String::new();
    let first = packets.first().map_or(0, |&(t, _)| t);
    // The PC of the last access each comparator matched
    let mut pcs = [None; 4];
    let mut writes = vec![0u64; values.len()];
    // The times of each value's first and last write
    let mut spans: Vec<Option<(u64, u64)>> = vec![None; values.len()];
    let mut reads = vec![0u64; values.len()];
    let mut writers: Vec<BTreeMap<String, u64>> = vec![BTreeMap::new(); values.len()];
    let mut overflows = 0;
    for &(time, packet) in packets {
        match packet {
            Packet::Pc { unit, pc } => pcs[usize::from(unit)] = Some(pc),
            Packet::Data { unit, write, value } => {
                let unit = usize::from(unit);
                let Some(traced) = values.get(unit) else {
                    continue;
                };
                let pc = pcs[unit].take();
                if !write {
                    reads[unit] += 1;
                    continue;
                }
                writes[unit] += 1;
                let span = spans[unit].get_or_insert((time, time));
                span.1 = time;
                let writer = pc.map(|pc| {
                    functions
                        .as_ref()
                        .and_then(|f| f.name_at(u64::from(pc)))
                        .map(crate::tasks::short_name)
                        .unwrap_or_else(|| format!("{pc:#010x}"))
                });
                *writers[unit]
                    .entry(writer.unwrap_or_else(|| "unknown".into()))
                    .or_default() += 1;
                let size = value::size_of(traced.scalar);
                let ms = (time - first) as f64 / hz * 1e3;
                let _ = writeln!(
                    out,
                    "{ms:10.3} ms  {}: {} = {}{}",
                    traced.label,
                    traced.scalar,
                    value::decode(traced.scalar, &value.to_le_bytes()[..size]),
                    traced.unit
                );
            }
            Packet::Overflow => overflows += 1,
        }
    }
    let seconds = elapsed.as_secs_f64();
    for (unit, traced) in values.iter().enumerate() {
        let from = writers[unit]
            .iter()
            .map(|(name, n)| format!("{name} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(out, "{}: {} writes", traced.label, writes[unit]);
        // The comparators start one after another, so each value's rate is over
        // its own writes.
        if let Some((first, last)) = spans[unit].filter(|(first, last)| last > first) {
            let rate = (writes[unit] - 1) as f64 * hz / (last - first) as f64;
            let _ = write!(out, ", {rate:.0} per second");
        }
        if reads[unit] > 0 {
            let _ = write!(out, ", {} reads", reads[unit]);
        }
        if !from.is_empty() {
            let _ = write!(out, ", from {from}");
        }
        out.push('\n');
    }
    if overflows > 0 {
        let _ = writeln!(
            out,
            "the trace overflowed {overflows} times, so some accesses are missing"
        );
    }
    let _ = writeln!(out, "{:.2} s at {:.0} MHz", seconds, hz / 1e6);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_data_trace_packets() {
        // Comparator 1: a PC packet, a write of 0x3f800000, then a local timestamp
        // of 300 cycles.
        let bytes = [
            0x57, 0x34, 0x12, 0x00, 0x08, // PC, ID 10
            0x8F, 0x00, 0x00, 0x80, 0x3F, // write, ID 17
            0xC0, 0xAC, 0x02, // timestamp 300
            0x70, 0x20, // overflow, then a timestamp of 2
            0xB4, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00, // a global timestamp from the board
            0x8F, 0x01, 0x00, 0x00, 0x00, 0x20, // another write, at 304
        ];
        let mut out = Vec::new();
        let mut decoder = Decoder::default();
        // Split mid-packet, as a drain can.
        decoder.feed(&bytes[..7], &mut out);
        decoder.feed(&bytes[7..], &mut out);
        assert_eq!(
            out,
            [
                (
                    300,
                    Packet::Pc {
                        unit: 1,
                        pc: 0x0800_1234
                    }
                ),
                (
                    300,
                    Packet::Data {
                        unit: 0,
                        write: true,
                        value: 0x3F80_0000
                    }
                ),
                (302, Packet::Overflow),
                (
                    304,
                    Packet::Data {
                        unit: 0,
                        write: true,
                        value: 1
                    }
                ),
            ]
        );
    }

    #[test]
    fn extracts_itm_bytes_from_frames() {
        // The start of a frame from the board: ID 13, a PC packet, a write and a
        // timestamp. Byte 15 holds the low bits of the even data bytes.
        let frame = [
            0x1b, 0x47, 0x92, 0x06, 0x00, 0x08, 0x8e, 0x00, 0x00, 0xc3, 0x3a, 0xc0, 0xfe, 0x88,
            0x7a, 0x48,
        ];
        let mut frames = Frames::default();
        // Split mid-frame, as a drain can.
        assert!(frames.feed(&frame[..5]).is_empty());
        assert_eq!(
            frames.feed(&frame[5..]),
            [
                0x47, 0x92, 0x06, 0x00, 0x08, 0x8f, 0x00, 0x00, 0xc3, 0x3a, 0xc0, 0xff, 0x88, 0x7a
            ]
        );
    }
}
