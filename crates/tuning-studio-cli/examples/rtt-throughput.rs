//! Measures raw probe-rs RTT transport against log-lab's rtt-throughput binary.
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use probe_rs::{
    Permissions,
    probe::{DebugProbeSelector, list::Lister},
    rtt::Rtt,
};
use serde_json::json;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    probe: String,
    #[arg(long, default_value = "STM32H723VGTx")]
    chip: String,
    #[arg(long)]
    speed: u32,
    #[arg(long)]
    rtt_address: String,
    #[arg(long, default_value_t = 10)]
    seconds: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (1..=60).contains(&args.seconds),
        "seconds must be between 1 and 60"
    );
    let address = u64::from_str_radix(args.rtt_address.trim_start_matches("0x"), 16)?;
    let selector: DebugProbeSelector = args.probe.parse()?;
    let info = Lister::new()
        .list_all()
        .into_iter()
        .find(|info| selector.matches_probe(info))
        .context("probe not found")?;
    let mut probe = info.open()?;
    let actual_speed = probe.set_speed(args.speed)?;
    let mut session = probe.attach(args.chip.as_str(), Permissions::default())?;
    let mut core = session.core(0)?;
    let before = format!("{:?}", core.status()?);
    let mut rtt = Rtt::attach_at(&mut core, address)?;
    let channel = rtt.up_channel(0).context("no up channel 0")?;
    let channel_name = channel.name().unwrap_or("").to_owned();
    ensure!(
        channel_name == "tuning-benchmark",
        "refusing unexpected channel {channel_name:?}"
    );
    let capacity = channel.buffer_size();
    let mut buffer = vec![0u8; 16384];
    let warmup = Instant::now();
    while warmup.elapsed() < Duration::from_secs(1) {
        channel.read(&mut core, &mut buffer)?;
    }
    let mut packets = Packets::default();
    let mut bytes = 0u64;
    let mut reads = 0u64;
    let mut empty = 0u64;
    let mut durations = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(args.seconds) {
        let call = Instant::now();
        let count = channel.read(&mut core, &mut buffer)?;
        durations.push(call.elapsed().as_secs_f64() * 1000.0);
        reads += 1;
        bytes += count as u64;
        if count == 0 {
            empty += 1;
        }
        packets.push(&buffer[..count]);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let after = format!("{:?}", core.status()?);
    durations.sort_by(f64::total_cmp);
    let p95 = durations[(durations.len() - 1) * 95 / 100];
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "requested_khz": args.speed, "actual_khz": actual_speed,
            "seconds": elapsed, "bytes": bytes, "bytes_per_second": bytes as f64 / elapsed,
            "reads": reads, "empty_reads": empty, "read_p95_ms": p95,
            "read_max_ms": durations.last(), "buffer_bytes": capacity, "channel": channel_name,
            "packets": packets.count, "sequence_gaps": packets.gaps,
            "corrupt_packets": packets.corrupt, "resync_bytes": packets.resync,
            "core_before": before, "core_after": after,
        }))?
    );
    Ok(())
}

#[derive(Default)]
struct Packets {
    pending: Vec<u8>,
    previous: Option<u32>,
    count: u64,
    gaps: u64,
    corrupt: u64,
    resync: u64,
}

impl Packets {
    fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let mut offset = 0;
        while self.pending.len() - offset >= 256 {
            let packet = &self.pending[offset..offset + 256];
            if &packet[..4] != b"TSRT" {
                offset += 1;
                self.resync += 1;
                continue;
            }
            if packet
                .iter()
                .enumerate()
                .skip(12)
                .any(|(i, byte)| *byte != i as u8)
            {
                self.corrupt += 1;
            }
            let sequence = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
            if let Some(previous) = self.previous {
                self.gaps += sequence.wrapping_sub(previous.wrapping_add(1)) as u64;
            }
            self.previous = Some(sequence);
            self.count += 1;
            offset += 256;
        }
        self.pending.drain(..offset);
    }
}

#[cfg(test)]
mod tests {
    use super::Packets;

    #[test]
    fn fragmented_packets_retain_framing_and_detect_missing_or_corrupt_data() {
        let mut bytes = Vec::new();
        for seq in [4u32, 5, 7] {
            let mut packet = [0u8; 256];
            packet[..4].copy_from_slice(b"TSRT");
            packet[4..8].copy_from_slice(&seq.to_le_bytes());
            for (index, byte) in packet.iter_mut().enumerate().skip(12) {
                *byte = index as u8;
            }
            if seq == 7 {
                packet[100] = 0;
            }
            bytes.extend_from_slice(&packet);
        }
        let mut decoder = Packets::default();
        for part in bytes.chunks(93) {
            decoder.push(part);
        }
        assert_eq!(decoder.count, 3);
        assert_eq!(decoder.gaps, 1);
        assert_eq!(decoder.corrupt, 1);
        assert!(decoder.pending.is_empty());
    }
}
