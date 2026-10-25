//! RTT log lab for the STM32H723: defmt frames on up channel 0 and plain text on
//! up channel 1, so `tuning-studio run` and `attach` can be checked against both.
//! It runs from the reset 64 MHz HSI clock with data in DTCM.
//!
//! Every 500 ms it logs a defmt line at a rotating level and a text line. Setting
//! `PANIC` to a nonzero value from the host panics on the next tick.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};
use panic_probe as _;
use rtt_target::{rprintln, rtt_init, set_defmt_channel, set_print_channel};

/// Milliseconds since startup, also the defmt timestamp
static MILLIS: AtomicU32 = AtomicU32::new(0);
/// Write a nonzero value to panic on the next tick.
#[used]
static PANIC: AtomicU32 = AtomicU32::new(0);

defmt::timestamp!("{=u32:ms}", MILLIS.load(Relaxed));

#[entry]
fn main() -> ! {
    let channels = rtt_init! {
        up: {
            0: { size: 1024, name: "defmt" }
            1: { size: 512, name: "Terminal" }
        }
    };
    set_defmt_channel(channels.up.0);
    set_print_channel(channels.up.1);
    defmt::info!("log lab started");
    rprintln!("log lab started on the text channel");

    let mut core = cortex_m::Peripherals::take().unwrap();
    core.SYST.set_clock_source(SystClkSource::Core);
    core.SYST.set_reload(64_000 - 1);
    core.SYST.clear_current();
    core.SYST.enable_interrupt();
    core.SYST.enable_counter();

    let mut last = 0;
    let mut tick = 0u32;
    loop {
        let now = MILLIS.load(Relaxed);
        if now.wrapping_sub(last) < 500 {
            core::hint::spin_loop();
            continue;
        }
        last = now;
        tick += 1;
        if PANIC.load(Relaxed) != 0 {
            panic!("PANIC was set to {}", PANIC.load(Relaxed));
        }
        match tick % 4 {
            0 => defmt::debug!("tick {=u32}", tick),
            1 => defmt::info!("tick {=u32}, uptime {=u32} ms", tick, now),
            2 => defmt::warn!("tick {=u32} is even", tick),
            _ => defmt::error!("tick {=u32}: values {=[u8]:?}", tick, [1u8, 2, 3][..]),
        }
        rprintln!("text tick {}", tick);
    }
}

#[exception]
fn SysTick() {
    MILLIS.fetch_add(1, Relaxed);
}
