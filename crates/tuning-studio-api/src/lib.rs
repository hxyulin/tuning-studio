//! Values the host reads and parameters it tunes, at fixed addresses and with
//! layouts the host reads from DWARF.
//!
//! - [`Watch`] and [`watch!`] keep a copy of a value for the host to read.
//!   Only the firmware writes it.
//! - [`Param`] is a value the host asks to change. The firmware applies the
//!   request in [`Param::get`], within the declared range and step, and replies.
//!
//! # Layout contract
//!
//! The host finds both types by name in the ELF's DWARF and reads their fields by
//! name, so field offsets may change. Renaming a field or changing what it means
//! is a format change.
#![no_std]

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering, fence};

/// Copy `value` into a static named `name` for the host to read.
///
/// ```
/// # fn control(target: f32, measured: f32) {
/// let error = target - measured;
/// tuning_studio_api::watch!("yaw.error": f32 = error);
/// # }
/// ```
///
/// The name is the static's symbol, so it must be unique in the firmware. A
/// repeated name fails to link. Each use is one static: a function called from
/// several tasks shares it, and the last call wins. The value's type cannot
/// depend on a generic parameter.
#[macro_export]
macro_rules! watch {
    ($name:literal: $type:ty = $value:expr) => {{
        #[unsafe(export_name = $name)]
        static WATCH: $crate::Watch<$type> = $crate::Watch::new();
        WATCH.set($value);
    }};
}

/// A copy of a value for the host to read.
///
/// A probe reads memory while the firmware runs, so it could see half of a
/// write. Two counters around the value let the host tell. [`Watch::set`] bumps
/// `start`, writes the value, then copies `start` to `end`. A probe reads upward
/// from `end` to `start`, so a read where both are equal saw one whole write.
/// Both are 0 until the first write.
#[repr(C)]
pub struct Watch<T> {
    end: AtomicU32,
    value: UnsafeCell<MaybeUninit<T>>,
    start: AtomicU32,
}

// SAFETY: `set` writes the value inside a critical section, and nothing in the
// firmware reads it.
unsafe impl<T: Copy + Send> Sync for Watch<T> {}

impl<T: Copy> Watch<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            end: AtomicU32::new(0),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            start: AtomicU32::new(0),
        }
    }

    pub fn set(&self, value: T) {
        let this = escape(self);
        critical_section::with(|_| {
            let count = match this.start.load(Ordering::Relaxed).wrapping_add(1) {
                0 => 1,
                count => count,
            };
            this.start.store(count, Ordering::Relaxed);
            // The probe must see `start` change before the value does, and the
            // value before `end`.
            fence(Ordering::Release);
            // SAFETY: the critical section keeps other writers out.
            unsafe { this.value.get().write_volatile(MaybeUninit::new(value)) };
            fence(Ordering::Release);
            this.end.store(count, Ordering::Relaxed);
        });
    }
}

impl<T: Copy> Default for Watch<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// How the firmware answered the host's last request, in [`Param`]'s `reply`
pub mod reply {
    /// No request yet
    pub const NONE: u8 = 0;
    pub const APPLIED: u8 = 1;
    /// Outside the range, so the nearest end of it was applied instead
    pub const CLAMPED: u8 = 2;
    /// Not a value of the parameter's type, such as a NaN or a `bool` of 2
    pub const REFUSED: u8 = 3;
}

/// A parameter the host can ask to change.
///
/// ```
/// use tuning_studio_api::Param;
///
/// static YAW_KP: Param<f32> = Param::new(1.2).range(0.0, 10.0).max_step(0.1).unit("1/s");
///
/// fn control_tick() {
///     let kp = YAW_KP.get();
///     // ...
/// }
/// ```
///
/// The host never writes the value the firmware runs with. It writes
/// `request`, then a new `request_id`. The next [`Param::get`] checks the
/// request, sets `target`, and writes the result to `reply` and the id to
/// `reply_id`. Each `get` then moves `value` toward `target` by at most
/// `max_step`. The host waits for the reply before sending another request.
#[repr(C)]
pub struct Param<T> {
    /// What the firmware runs with; only `get` writes it
    value: UnsafeCell<T>,
    /// What `value` moves toward
    target: UnsafeCell<T>,
    /// Written by the host: the value's bytes, zero-extended to 32 bits
    request: AtomicU32,
    /// Written by the host after `request`
    request_id: AtomicU32,
    /// Written by the firmware once it has handled `request_id`
    reply_id: AtomicU32,
    /// One of the [`reply`] constants
    reply: AtomicU8,
    default: T,
    min: T,
    max: T,
    max_step: T,
    unit: &'static str,
}

// SAFETY: `get` reads and writes `value` and `target` only inside a critical
// section; the host's fields are atomics.
unsafe impl<T: Value + Send> Sync for Param<T> {}

impl<T: Value> Param<T> {
    /// A parameter that starts at `default`, with no range or step limit.
    #[must_use]
    pub const fn new(default: T) -> Self {
        Self {
            value: UnsafeCell::new(default),
            target: UnsafeCell::new(default),
            request: AtomicU32::new(0),
            request_id: AtomicU32::new(0),
            reply_id: AtomicU32::new(0),
            reply: AtomicU8::new(reply::NONE),
            default,
            min: T::MIN,
            max: T::MAX,
            max_step: T::UNLIMITED,
            unit: "",
        }
    }

    /// Clamp requests to `min..=max`.
    #[must_use]
    pub const fn range(mut self, min: T, max: T) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Move by at most `max_step` per [`Param::get`]. Ignored for `bool`.
    #[must_use]
    pub const fn max_step(mut self, max_step: T) -> Self {
        self.max_step = max_step;
        self
    }

    /// The unit the host shows, such as `"rad/s"`.
    #[must_use]
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    /// Apply a pending request, step toward the target, and return the value.
    /// Call it once per control tick, where a change is safe to take.
    pub fn get(&self) -> T {
        let this = escape(self);
        critical_section::with(|_| {
            // SAFETY: only `get` touches `value` and `target`, inside the critical section.
            let (value, target) = unsafe { (&mut *this.value.get(), &mut *this.target.get()) };
            let id = this.request_id.load(Ordering::Acquire);
            if id != this.reply_id.load(Ordering::Relaxed) {
                let reply = match T::from_request(this.request.load(Ordering::Relaxed)) {
                    Some(request) => {
                        let (limited, reply) = T::limit(request, this.min, this.max);
                        *target = limited;
                        reply
                    }
                    None => reply::REFUSED,
                };
                this.reply.store(reply, Ordering::Relaxed);
                this.reply_id.store(id, Ordering::Release);
            }
            *value = T::toward(*value, *target, this.max_step);
            *value
        })
    }

    /// The value as of the last [`Param::get`].
    pub fn last(&self) -> T {
        // SAFETY: a copy taken inside the critical section `get` writes in.
        let this = escape(self);
        critical_section::with(|_| unsafe { *this.value.get() })
    }

    pub const fn default_value(&self) -> T {
        self.default
    }
}

/// Hide where `value` lives from the optimizer. Seeing every access to a static
/// at a constant offset, LLVM may split it into separate variables or drop the
/// fields the firmware never reads, and the host would no longer find them where
/// the DWARF says. In a build of the lab with LTO, two `Param`s lost their storage.
fn escape<T>(value: &T) -> &T {
    core::hint::black_box(value)
}

/// The types a [`Param`] can hold: at most 32 bits, so the host reads them
/// whole.
pub trait Value: Copy + sealed::Sealed {
    #[doc(hidden)]
    const MIN: Self;
    #[doc(hidden)]
    const MAX: Self;
    #[doc(hidden)]
    const UNLIMITED: Self;
    #[doc(hidden)]
    fn from_request(bits: u32) -> Option<Self>;
    #[doc(hidden)]
    fn limit(request: Self, min: Self, max: Self) -> (Self, u8);
    #[doc(hidden)]
    fn toward(from: Self, to: Self, max_step: Self) -> Self;
}

mod sealed {
    pub trait Sealed {}
}

fn limit<T: PartialOrd>(request: T, min: T, max: T) -> (T, u8) {
    if request < min {
        (min, reply::CLAMPED)
    } else if request > max {
        (max, reply::CLAMPED)
    } else {
        (request, reply::APPLIED)
    }
}

macro_rules! integer {
    ($($type:ty),*) => {$(
        impl sealed::Sealed for $type {}
        impl Value for $type {
            const MIN: Self = <$type>::MIN;
            const MAX: Self = <$type>::MAX;
            const UNLIMITED: Self = <$type>::MAX;
            fn from_request(bits: u32) -> Option<Self> {
                // The host zero-extends the value's bytes, so truncating recovers them.
                Some(bits as $type)
            }
            fn limit(request: Self, min: Self, max: Self) -> (Self, u8) {
                limit(request, min, max)
            }
            fn toward(from: Self, to: Self, max_step: Self) -> Self {
                // A signed range can be wider than the type's largest step.
                if max_step == Self::UNLIMITED {
                    return to;
                }
                let step = i64::from(max_step).abs();
                let next = i64::from(from) + (i64::from(to) - i64::from(from)).clamp(-step, step);
                next as $type
            }
        }
    )*};
}

integer!(i8, i16, i32, u8, u16, u32);

impl sealed::Sealed for f32 {}
impl Value for f32 {
    const MIN: Self = f32::NEG_INFINITY;
    const MAX: Self = f32::INFINITY;
    const UNLIMITED: Self = f32::INFINITY;
    fn from_request(bits: u32) -> Option<Self> {
        Some(f32::from_bits(bits)).filter(|value| !value.is_nan())
    }
    fn limit(request: Self, min: Self, max: Self) -> (Self, u8) {
        limit(request, min, max)
    }
    fn toward(from: Self, to: Self, max_step: Self) -> Self {
        let change = to - from;
        if change.abs() <= max_step.abs() {
            to
        } else {
            from + max_step.abs().copysign(change)
        }
    }
}

impl sealed::Sealed for bool {}
impl Value for bool {
    const MIN: Self = false;
    const MAX: Self = true;
    const UNLIMITED: Self = true;
    fn from_request(bits: u32) -> Option<Self> {
        match bits {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn limit(request: Self, min: Self, max: Self) -> (Self, u8) {
        limit(request, min, max)
    }
    fn toward(_: Self, to: Self, _: Self) -> Self {
        to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Send a request the way the host does.
    fn request<T: Value>(param: &Param<T>, bits: u32) -> u32 {
        let id = param.request_id.load(Ordering::Relaxed) + 1;
        param.request.store(bits, Ordering::Relaxed);
        param.request_id.store(id, Ordering::Release);
        id
    }

    fn reply<T: Value>(param: &Param<T>) -> (u32, u8) {
        (
            param.reply_id.load(Ordering::Acquire),
            param.reply.load(Ordering::Relaxed),
        )
    }

    #[test]
    fn applies_requests_in_get_within_range_and_step() {
        let kp = Param::new(1.0f32).range(0.0, 10.0).max_step(0.5);
        assert_eq!(kp.get(), 1.0);
        assert_eq!(reply(&kp), (0, reply::NONE));

        let id = request(&kp, 2.0f32.to_bits());
        // Nothing changes until the firmware calls `get`.
        assert_eq!(kp.last(), 1.0);
        assert_eq!(kp.get(), 1.5);
        assert_eq!(reply(&kp), (id, reply::APPLIED));
        assert_eq!(kp.get(), 2.0);
        assert_eq!(kp.get(), 2.0);

        let id = request(&kp, 20.0f32.to_bits());
        kp.get();
        assert_eq!(reply(&kp), (id, reply::CLAMPED));
        let id = request(&kp, f32::NAN.to_bits());
        kp.get();
        assert_eq!(reply(&kp), (id, reply::REFUSED));
        // A refused request leaves the target alone.
        for _ in 0..100 {
            kp.get();
        }
        assert_eq!(kp.last(), 10.0);
        assert_eq!(kp.default_value(), 1.0);
    }

    #[test]
    fn integers_and_bools() {
        let offset = Param::new(0i16).range(-100, 100).max_step(30);
        request(&offset, (-50i16) as u16 as u32);
        assert_eq!(offset.get(), -30);
        assert_eq!(offset.get(), -50);

        let wide = Param::new(i32::MIN);
        request(&wide, i32::MAX as u32);
        assert_eq!(wide.get(), i32::MAX);

        let enable = Param::new(false);
        request(&enable, 2);
        enable.get();
        assert_eq!(reply(&enable).1, reply::REFUSED);
        request(&enable, 1);
        assert!(enable.get());
    }

    #[test]
    fn watch_counters_match_after_each_write() {
        static ERROR: Watch<f64> = Watch::new();
        assert_eq!(ERROR.start.load(Ordering::Relaxed), 0);
        ERROR.set(0.25);
        ERROR.set(-1.5);
        assert_eq!(ERROR.start.load(Ordering::Relaxed), 2);
        assert_eq!(ERROR.end.load(Ordering::Relaxed), 2);
        assert_eq!(unsafe { (*ERROR.value.get()).assume_init() }, -1.5);
    }
}
