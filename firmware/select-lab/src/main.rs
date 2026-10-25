//! Stress lab for reading async state on the STM32H723. One Embassy task awaits
//! `select(sine(), cosine())`. Each future counts its own iterations, looks up
//! the sine or cosine of its count's phase in a table, and yields, so the
//! executor switches between them every iteration. Each keeps `count` and `value` across its
//! `.await`, which puts them in the task's future, and also publishes both with
//! `watch!`.
//!
//! `value` is always `f(phase)` with `phase = count % 1024 * 2π / 1024`, so a
//! host can check any read of the pair.
//!
//! It runs from the reset 64 MHz HSI clock with data in DTCM and the instruction
//! cache on.
#![no_std]
#![no_main]

use core::f32::consts::TAU;
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_futures::yield_now;
use tuning_studio_api::{params, watch};

params! {
    /// Extra cycles each future spins per iteration, to slow the switching down.
    /// The futures read both parameters once per 1024 iterations.
    static SPIN: u32 = 0, unit = "cycles";
    /// Publish the `watch!` values every this many iterations
    static WATCH_EVERY: u32 = 1, range = 1..=u32::MAX;
}

const STEPS: u32 = 1024;

#[derive(Clone, Copy)]
struct Sample {
    count: u32,
    value: f32,
}

fn phase(count: u32) -> f32 {
    (count % STEPS) as f32 * (TAU / STEPS as f32)
}

/// `sin` of each phase, filled at startup, so an iteration is a table lookup
static mut SINE: [f32; STEPS as usize] = [0.0; STEPS as usize];

fn sine_at(count: u32) -> f32 {
    // SAFETY: written only before the executor starts.
    unsafe { (*core::ptr::addr_of!(SINE))[(count % STEPS) as usize] }
}

/// The parameters, read once per turn of the phase so the loop stays short
struct Settings {
    spin: u32,
    watch_every: u32,
}

fn settings(count: u32, current: &mut Settings) {
    if count.is_multiple_of(STEPS) {
        *current = Settings {
            spin: SPIN.get(),
            watch_every: WATCH_EVERY.get(),
        };
    }
}

async fn sine() -> ! {
    let mut sample = Sample {
        count: 0,
        value: 0.0,
    };
    let mut current = Settings {
        spin: 0,
        watch_every: 1,
    };
    loop {
        sample.count = sample.count.wrapping_add(1);
        sample.value = sine_at(sample.count);
        settings(sample.count, &mut current);
        if current.spin > 0 {
            cortex_m::asm::delay(current.spin);
        }
        if sample.count.is_multiple_of(current.watch_every) {
            watch!("waves.sine": Sample = sample);
        }
        yield_now().await;
        // Keeps `sample` live across the await, so it sits in the future.
        core::hint::black_box(&sample);
    }
}

async fn cosine() -> ! {
    let mut sample = Sample {
        count: 0,
        value: 1.0,
    };
    let mut current = Settings {
        spin: 0,
        watch_every: 1,
    };
    loop {
        sample.count = sample.count.wrapping_add(1);
        sample.value = sine_at(sample.count.wrapping_add(STEPS / 4));
        settings(sample.count, &mut current);
        if current.spin > 0 {
            cortex_m::asm::delay(current.spin);
        }
        if sample.count.is_multiple_of(current.watch_every) {
            watch!("waves.cosine": Sample = sample);
        }
        yield_now().await;
        core::hint::black_box(&sample);
    }
}

#[embassy_executor::task]
async fn waves() {
    select(sine(), cosine()).await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // SAFETY: the executor has not started the futures that read it.
    for (i, value) in unsafe { &mut *core::ptr::addr_of_mut!(SINE) }
        .iter_mut()
        .enumerate()
    {
        *value = libm::sinf(phase(i as u32));
    }
    // Without the instruction cache, each fetch from flash waits the reset default
    // of 7 cycles.
    // SAFETY: nothing else uses the SCB.
    unsafe { cortex_m::Peripherals::steal() }
        .SCB
        .enable_icache();
    spawner.spawn(waves().unwrap());
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
