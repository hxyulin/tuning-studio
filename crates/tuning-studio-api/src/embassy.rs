//! Task profiling through the Embassy 0.10 executor trace hooks.
//!
//! Enable the `embassy` feature, call [`init`] with a free-running clock, then
//! call [`publish`] periodically from one task. The host reads
//! `tuning_studio.tasks` through the normal `values`, `read`, and `watch` commands.
//! This module owns all seven trace hook symbols; do not link another provider.
//!
//! Poll time is elapsed time, including interrupts and nested executor polls.
//! It is not exclusive CPU time. A poll must last less than one clock wrap.
//! Counters wrap at 32 bits; hosts must difference samples with wrapping
//! arithmetic and match both IDs and `generation`. IDs are opaque, not a stable
//! promise of task addresses. The critical-section implementation must serialize
//! hooks across every executor/core in use, and the clock must be shared by them.
//!
//! Trace hooks neither suspend tasks nor make their locals writable. A published
//! profile is consistent, but does not certify a separate read of future storage.

use core::cell::RefCell;
use core::fmt;
use core::num::NonZeroU32;

use critical_section::Mutex;

use crate::Watch;

/// Maximum number of simultaneously tracked task instances.
pub const MAX_TASKS: usize = 32;

/// Latest published profile, discovered as a normal watch value by the host.
#[cfg_attr(target_os = "none", unsafe(link_section = ".data.tuning_studio"))]
#[unsafe(export_name = "tuning_studio.tasks")]
pub static PROFILE: Watch<Profile> = Watch::new();

/// A consistent snapshot of the task counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// Profile format version, currently 1.
    pub version: u32,
    /// Clock ticks per second; zero before initialization.
    pub tick_hz: u32,
    /// Clock reading at snapshot time, wrapping at 32 bits.
    pub sampled_at: u32,
    /// Spawns omitted because all slots were occupied, wrapping at 32 bits.
    pub untracked: u32,
    /// Slots with `generation == 0` have never been used. Completed tasks remain
    /// visible until their slot is reused.
    pub tasks: [Task; MAX_TASKS],
}

/// Counters for one task instance. Times are in the clock's ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Task {
    /// Opaque executor ID supplied by Embassy.
    pub executor: u32,
    /// Opaque task ID supplied by Embassy.
    pub id: u32,
    /// Instance number, changed on slot reuse. Zero denotes an unused slot.
    pub generation: u32,
    /// Whether this task has not yet completed.
    pub alive: bool,
    /// Whether a poll is in progress. Nested executor polls may also be running.
    pub running: bool,
    /// Whether a ready notification has occurred since the last poll began.
    pub ready: bool,
    /// Number of completed polls, including those before clock initialization.
    pub polls: u32,
    /// Number of completed polls with a valid start and end timestamp.
    pub timed_polls: u32,
    /// Sum of elapsed times of completed, timed polls. Includes preemption.
    pub ticks: u32,
    /// Longest completed, timed poll since this instance was spawned.
    pub max_ticks: u32,
    /// Ready notifications, which can be coalesced into a single poll.
    pub wakes: u32,
}

impl Task {
    const EMPTY: Self = Self {
        executor: 0,
        id: 0,
        generation: 0,
        alive: false,
        running: false,
        ready: false,
        polls: 0,
        timed_polls: 0,
        ticks: 0,
        max_ticks: 0,
        wakes: 0,
    };
}

#[derive(Clone, Copy)]
struct Slot {
    task: Task,
    started: Option<u32>,
}

struct Recorder {
    clock: Option<fn() -> u32>,
    tick_hz: u32,
    generation: u32,
    untracked: u32,
    slots: [Slot; MAX_TASKS],
}

static RECORDER: Mutex<RefCell<Recorder>> = Mutex::new(RefCell::new(Recorder::new()));

/// The profiler clock was already configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyInitialized;

impl fmt::Display for AlreadyInitialized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the Embassy profiler clock is already initialized")
    }
}

impl core::error::Error for AlreadyInitialized {}

/// Configure a free-running wrapping clock, without resetting task counters.
///
/// On Cortex-M with DWT cycle counting, enable the counter through your board's
/// peripheral API and pass its read function and core frequency. This module
/// does not configure debug hardware. Keep the clock frequency fixed afterward.
/// `now` runs inside a critical section. It must be short, must not block, and
/// must not call this module or other code that borrows its recorder.
///
/// # Errors
/// Returns [`AlreadyInitialized`] if called more than once. Polls started before
/// initialization still count, but do not contribute time.
pub fn init(now: fn() -> u32, tick_hz: NonZeroU32) -> Result<(), AlreadyInitialized> {
    critical_section::with(|cs| {
        let mut recorder = RECORDER.borrow(cs).borrow_mut();
        if recorder.clock.is_some() {
            return Err(AlreadyInitialized);
        }
        recorder.clock = Some(now);
        recorder.tick_hz = tick_hz.get();
        Ok(())
    })
}

/// Read the counters together under a critical section.
///
/// No allocations or probe transfers occur. Copying the fixed table briefly
/// delays interrupts according to the application's critical-section provider.
#[must_use]
pub fn snapshot() -> Profile {
    critical_section::with(|cs| RECORDER.borrow(cs).borrow().snapshot())
}

/// Publish a profile for the host. Call periodically from a single task.
///
/// A rate such as 10 Hz avoids copying the table on every poll. The host sees
/// the last publication, whose `sampled_at` records when it was captured.
/// This also retains the watch static in firmware that uses LTO.
pub fn publish() {
    PROFILE.set(snapshot());
}

impl Recorder {
    const fn new() -> Self {
        Self {
            clock: None,
            tick_hz: 0,
            generation: 0,
            untracked: 0,
            slots: [Slot {
                task: Task::EMPTY,
                started: None,
            }; MAX_TASKS],
        }
    }

    fn now(&self) -> Option<u32> {
        self.clock.map(|now| now())
    }

    fn snapshot(&self) -> Profile {
        Profile {
            version: 1,
            tick_hz: self.tick_hz,
            sampled_at: self.now().unwrap_or(0),
            untracked: self.untracked,
            tasks: self.slots.map(|slot| slot.task),
        }
    }

    fn find(
        &mut self,
        executor: u32,
        id: u32,
        matches: impl Fn(&Task) -> bool,
    ) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|slot| {
            slot.task.generation != 0
                && slot.task.executor == executor
                && slot.task.id == id
                && matches(&slot.task)
        })
    }

    fn spawn(&mut self, executor: u32, id: u32) {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| !s.task.alive && !s.task.running)
        else {
            self.untracked = self.untracked.wrapping_add(1);
            return;
        };
        self.generation = self.generation.wrapping_add(1).max(1);
        *slot = Slot {
            task: Task {
                executor,
                id,
                generation: self.generation,
                alive: true,
                ..Task::EMPTY
            },
            started: None,
        };
    }

    fn begin(&mut self, executor: u32, id: u32) {
        let now = self.now();
        if let Some(slot) = self.find(executor, id, |task| task.alive) {
            slot.task.running = true;
            slot.task.ready = false;
            slot.started = now;
        }
    }

    fn end(&mut self, executor: u32, id: u32) {
        let now = self.now();
        if let Some(slot) = self.find(executor, id, |task| task.running) {
            slot.task.running = false;
            slot.task.polls = slot.task.polls.wrapping_add(1);
            if let (Some(started), Some(now)) = (slot.started.take(), now) {
                let elapsed = now.wrapping_sub(started);
                slot.task.timed_polls = slot.task.timed_polls.wrapping_add(1);
                slot.task.ticks = slot.task.ticks.wrapping_add(elapsed);
                slot.task.max_ticks = slot.task.max_ticks.max(elapsed);
            }
        }
    }

    fn ready(&mut self, executor: u32, id: u32) {
        if let Some(slot) = self.find(executor, id, |task| task.alive) {
            slot.task.ready = true;
            slot.task.wakes = slot.task.wakes.wrapping_add(1);
        }
    }

    fn exit(&mut self, executor: u32, id: u32) {
        if let Some(slot) = self.find(executor, id, |task| task.alive) {
            // Embassy calls task_end before exec_end on a completing poll.
            // Keep the running slot until its matching exec_end has arrived.
            slot.task.alive = false;
            slot.task.ready = false;
        }
    }
}

fn record(event: impl FnOnce(&mut Recorder)) {
    critical_section::with(|cs| event(&mut RECORDER.borrow(cs).borrow_mut()));
}

// These symbols implement Embassy 0.10's extern "Rust" trace ABI. Keeping them
// private avoids offering the executor's hook ABI as a separate public Rust API.
#[unsafe(no_mangle)]
fn _embassy_trace_task_new(executor: u32, task: u32) {
    record(|recorder| recorder.spawn(executor, task));
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_end(executor: u32, task: u32) {
    record(|recorder| recorder.exit(executor, task));
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_exec_begin(executor: u32, task: u32) {
    record(|recorder| recorder.begin(executor, task));
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_exec_end(executor: u32, task: u32) {
    record(|recorder| recorder.end(executor, task));
}

#[unsafe(no_mangle)]
fn _embassy_trace_task_ready_begin(executor: u32, task: u32) {
    record(|recorder| recorder.ready(executor, task));
}

#[unsafe(no_mangle)]
fn _embassy_trace_poll_start(_executor: u32) {}

#[unsafe(no_mangle)]
fn _embassy_trace_executor_idle(_executor: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

    #[test]
    fn counts_a_completing_poll_and_reuses_its_slot() {
        static CLOCK: AtomicU32 = AtomicU32::new(u32::MAX - 9);
        let mut recorder = Recorder::new();
        recorder.clock = Some(|| CLOCK.load(Relaxed));
        recorder.spawn(1, 2);
        recorder.ready(1, 2);
        recorder.begin(1, 2);
        recorder.ready(1, 2); // A wake during polling queues the next poll.
        assert!(recorder.snapshot().tasks[0].ready);
        recorder.exit(1, 2);
        recorder.spawn(1, 3); // Cannot reuse the slot before exec_end.
        CLOCK.store(10, Relaxed);
        recorder.end(1, 2);
        let task = recorder.snapshot().tasks[0];
        assert_eq!(task.polls, 1, "the completing poll must count");
        assert_eq!(task.ticks, 20, "elapsed time must handle clock wrap");
        assert_eq!(task.max_ticks, 20);
        assert!(!task.alive);
        assert!(!task.running);
        recorder.spawn(1, 2);
        let respawned = recorder.snapshot().tasks[0];
        assert_ne!(respawned.generation, task.generation);
        assert_eq!(respawned.polls, 0, "respawning starts new counters");
    }

    #[test]
    fn nested_executors_keep_separate_inclusive_times() {
        static CLOCK: AtomicU32 = AtomicU32::new(0);
        let mut recorder = Recorder::new();
        recorder.clock = Some(|| CLOCK.load(Relaxed));
        recorder.spawn(1, 11);
        recorder.spawn(2, 22);
        recorder.begin(1, 11);
        CLOCK.store(10, Relaxed);
        recorder.begin(2, 22);
        CLOCK.store(30, Relaxed);
        recorder.end(2, 22);
        CLOCK.store(50, Relaxed);
        recorder.end(1, 11);
        let tasks = recorder.snapshot().tasks;
        assert_eq!(tasks[0].ticks, 50, "outer time includes preemption");
        assert_eq!(tasks[1].ticks, 20, "nested polls retain their own start");
    }

    #[test]
    fn initialization_mid_poll_does_not_invent_a_duration() {
        let mut recorder = Recorder::new();
        recorder.spawn(0, 0); // IDs are opaque; zero is not an empty marker.
        recorder.begin(0, 0);
        recorder.clock = Some(|| 500);
        recorder.end(0, 0);
        let task = recorder.snapshot().tasks[0];
        assert_eq!(task.polls, 1);
        assert_eq!(task.timed_polls, 0);
        assert_eq!(task.ticks, 0);
    }

    #[test]
    fn capacity_exhaustion_is_visible_and_completed_slots_can_be_reused() {
        let mut recorder = Recorder::new();
        for id in 0..MAX_TASKS as u32 {
            recorder.spawn(1, id);
        }
        recorder.spawn(1, 100);
        assert_eq!(recorder.snapshot().untracked, 1);
        recorder.exit(1, 5);
        recorder.spawn(2, 100);
        assert_eq!(recorder.snapshot().tasks[5].executor, 2);
        assert_eq!(recorder.snapshot().tasks[5].id, 100);
    }
}
