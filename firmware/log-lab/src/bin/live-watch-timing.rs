//! Measures SysTick service gaps against TIM2 while sleeping between interrupts.
//! TIM2 keeps running during debug halts; no RTT producer is required.
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};

// Request epoch, acknowledged epoch, max IRQ gap (us), ticks, last timer, last gap,
// mode (0 sleeps, 1 spins), then padding for different Live Watch read sizes.
#[unsafe(no_mangle)]
static WATCH_WORDS: [AtomicU32; 256] = [const { AtomicU32::new(0) }; 256];
static PREVIOUS: AtomicU32 = AtomicU32::new(0);

#[entry]
fn main() -> ! {
    // SAFETY: This standalone STM32H723 fixture owns TIM2, RCC, and DBGMCU.
    // Register offsets match stm32-metapac 21's stm32h723vg PAC. The 64 MHz
    // reset clock is not reconfigured. Clearing DBG_TIM2_STOP keeps TIM2 running
    // during halts; no timer outputs or board pins are enabled.
    unsafe {
        let enable = 0x5802_44e8 as *mut u32;
        write_volatile(enable, read_volatile(enable) | 1);
        let _ = read_volatile(enable);
        let freeze = 0x5c00_103c as *mut u32;
        write_volatile(freeze, read_volatile(freeze) & !1);
        write_volatile(0x4000_0000 as *mut u32, 0); // CR1
        write_volatile(0x4000_0028 as *mut u32, 63); // PSC, 1 MHz timer ticks
        write_volatile(0x4000_002c as *mut u32, u32::MAX); // ARR
        write_volatile(0x4000_0014 as *mut u32, 1); // EGR, latch prescaler
        write_volatile(0x4000_0010 as *mut u32, 0); // SR
        write_volatile(0x4000_0000 as *mut u32, 1); // CR1, enable counter
    }
    let Some(mut core) = cortex_m::Peripherals::take() else {
        loop {
            cortex_m::asm::nop();
        }
    };
    PREVIOUS.store(timer(), Relaxed);
    core.SYST.set_clock_source(SystClkSource::Core);
    core.SYST.set_reload(64_000 - 1);
    core.SYST.clear_current();
    core.SYST.enable_interrupt();
    core.SYST.enable_counter();
    loop {
        if WATCH_WORDS[6].load(Relaxed) == 0 {
            cortex_m::asm::wfi();
        } else {
            core::hint::spin_loop();
        }
    }
}

fn timer() -> u32 {
    // SAFETY: TIM2 is initialized before this function is called and CNT is a
    // readable, aligned, 32-bit register. Reads do not change the counter.
    unsafe { read_volatile(0x4000_0024 as *const u32) }
}

#[exception]
fn SysTick() {
    let now = timer();
    let delta = now.wrapping_sub(PREVIOUS.swap(now, Relaxed));
    let request = WATCH_WORDS[0].load(Relaxed);
    if request != WATCH_WORDS[1].load(Relaxed) {
        WATCH_WORDS[2].store(0, Relaxed);
        WATCH_WORDS[1].store(request, Relaxed);
    } else {
        WATCH_WORDS[2].fetch_max(delta, Relaxed);
    }
    WATCH_WORDS[3].fetch_add(1, Relaxed);
    WATCH_WORDS[4].store(now, Relaxed);
    WATCH_WORDS[5].store(delta, Relaxed);
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    loop {
        cortex_m::asm::nop();
    }
}
