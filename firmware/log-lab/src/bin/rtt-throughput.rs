//! Saturates a nonblocking RTT channel with verifiable 256-byte packets.
//! Uses the reset 64 MHz HSI clock and uncached DTCM. No board peripherals are driven.
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};
use rtt_target::{ChannelMode, rtt_init};

static MILLIS: AtomicU32 = AtomicU32::new(0);

#[entry]
fn main() -> ! {
    let channels = rtt_init! {
        up: { 0: { size: 16384, mode: ChannelMode::NoBlockSkip, name: "tuning-benchmark" } }
    };
    let mut output = channels.up.0;
    output.set_mode(ChannelMode::NoBlockSkip);
    let Some(mut core) = cortex_m::Peripherals::take() else {
        loop {
            cortex_m::asm::nop();
        }
    };
    core.SYST.set_clock_source(SystClkSource::Core);
    core.SYST.set_reload(64_000 - 1);
    core.SYST.clear_current();
    core.SYST.enable_interrupt();
    core.SYST.enable_counter();

    let mut packet = [0u8; 256];
    packet[..4].copy_from_slice(b"TSRT");
    for (index, byte) in packet.iter_mut().enumerate().skip(12) {
        *byte = index as u8;
    }
    let mut sequence = 0u32;
    loop {
        packet[4..8].copy_from_slice(&sequence.to_le_bytes());
        packet[8..12].copy_from_slice(&MILLIS.load(Relaxed).to_le_bytes());
        if output.write(&packet) == packet.len() {
            sequence = sequence.wrapping_add(1);
        }
    }
}

#[exception]
fn SysTick() {
    MILLIS.fetch_add(1, Relaxed);
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    loop {
        cortex_m::asm::nop();
    }
}
