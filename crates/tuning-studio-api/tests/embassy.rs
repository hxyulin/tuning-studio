//! Exercise the exported hooks through an actual Embassy raw executor.
#![cfg(feature = "embassy")]

use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use tuning_studio_api::embassy;

static CLOCK: AtomicU32 = AtomicU32::new(0);

// The raw executor's wake notification. This test polls it explicitly.
#[unsafe(export_name = "__pender")]
fn pender(_context: *mut ()) {}

#[embassy_executor::task]
async fn finishes() {
    CLOCK.store(42, Relaxed);
}

#[test]
fn executor_hooks_count_the_final_poll_and_link_without_a_platform() {
    let executor = Box::leak(Box::new(embassy_executor::raw::Executor::new(
        core::ptr::null_mut(),
    )));
    embassy::init(
        || CLOCK.load(Relaxed),
        NonZeroU32::new(1_000_000).expect("the test clock frequency is nonzero"),
    )
    .expect("the test initializes the profiler once");
    assert!(embassy::init(|| 0, NonZeroU32::MIN).is_err());
    executor
        .spawner()
        .spawn(finishes().expect("the task pool has a free slot"));
    // SAFETY: only this thread polls this executor, and polling is not reentrant.
    unsafe { executor.poll() };
    embassy::publish();
    let profile = embassy::snapshot();
    let task = profile
        .tasks
        .iter()
        .find(|task| task.generation != 0)
        .expect("the executor called the task creation hook");
    assert_eq!(task.polls, 1, "task_end must not discard the final poll");
    assert_eq!(task.timed_polls, 1);
    assert_eq!(task.ticks, 42);
    assert!(!task.alive);
    assert!(!task.running);
    assert_eq!(profile.tick_hz, 1_000_000);
    assert_eq!(profile.sampled_at, 42);
}
