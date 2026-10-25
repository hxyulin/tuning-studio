//! Embassy fixture for studio-dwarf's task view: tasks with arguments, locals
//! held across `.await`, several `.await` points, and a pool of two, counted
//! by a copy of rm-embedded-rs' `rm-task-stats`.
#![no_std]
#![no_main]

use cortex_m as _; // links its critical-section implementation

use core::future::poll_fn;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Poll;

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

mod rm_task_stats;

pub static TICKS: Signal<CriticalSectionRawMutex, u32> = Signal::new();
pub static HANDLED: AtomicU32 = AtomicU32::new(0);

/// Pending once, then ready: a real `.await` that never blocks for long
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

pub mod blink {
    use super::*;

    #[embassy_executor::task]
    pub async fn blink_task(period: u32) {
        let mut count = 0u32;
        loop {
            count = count.wrapping_add(period);
            TICKS.signal(count);
            yield_now().await;
            if count % 3 == 0 {
                yield_now().await;
            }
        }
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn worker(id: u8) {
    let mut total = 0u32;
    loop {
        let tick = TICKS.wait().await;
        total = total.wrapping_add(tick + u32::from(id));
        HANDLED.store(total, Ordering::Relaxed);
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    rm_task_stats::init(64_000_000);
    spawner.spawn(blink::blink_task(3).unwrap());
    spawner.spawn(worker(0).unwrap());
    spawner.spawn(worker(1).unwrap());
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {}
}
