//! Values the host reads and parameters it tunes, at fixed addresses and with
//! layouts the host reads from DWARF.
//!
//! - [`Watch`] and [`watch!`] keep a copy of a value for the host to read.
//!   Only the firmware writes it. Anything [`Watchable`] can be watched: every
//!   `Copy` type, and structs that derive it.
//! - [`Param`] is a value the host asks to change. The firmware applies the
//!   request in [`Param::get`], within the declared range, and replies.
//! - [`watch_mut!`] publishes a scalar local and applies host edits through a
//!   temporary mutable reference. The local keeps its normal Rust ownership.
//! - The optional `embassy` feature profiles tasks through Embassy 0.10's trace
//!   hooks and publishes counters through the same watch interface.
//!
//! # Layout contract
//!
//! The host finds these types by name in the ELF's DWARF and reads their fields by
//! name, so field offsets may change. Renaming a field or changing what it means
//! is a format change.
#![no_std]

// The derive names this crate by its path, which this lets its own tests use.
extern crate self as tuning_studio_api;

mod watch_mut;
pub use watch_mut::WatchMut;

#[cfg(feature = "embassy")]
pub mod embassy;

#[cfg(feature = "derive")]
pub use tuning_studio_api_macros::Watchable;

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::RangeInclusive;
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
/// The value can be any [`Watchable`] type. Pass the value or a place, such as
/// `*motor` behind a reference; the macro borrows it and stores its snapshot.
///
/// `unit = "rad/s"` gives the unit the host shows after the value.
///
/// On bare-metal targets the static goes in `.data.tuning_studio`, with every
/// [`params!`] parameter, so the host can read them all in a few transfers.
/// Linker scripts that place `.data.*`, such as cortex-m-rt's, need no change.
/// `section = ".uncached"` places it in that section instead, which the
/// board's linker script must define. The section must be initialized at
/// boot; otherwise the host can show leftover bytes until the first write.
///
/// The name is the static's symbol, so it must be unique in the firmware. A
/// repeated name fails to link. Each use is one static: a function called from
/// several tasks shares it, and the last call wins. The value's type cannot
/// depend on a generic parameter.
#[macro_export]
macro_rules! watch {
    ($name:literal: $type:ty = $value:expr $(, unit = $unit:literal)?, section = $section:literal) => {{
        #[unsafe(link_section = $section)]
        #[unsafe(export_name = $name)]
        static WATCH: $crate::Watch<<$type as $crate::Watchable>::Snapshot> =
            $crate::Watch::new()$(.unit($unit))?;
        WATCH.set(<$type as $crate::Watchable>::snapshot(&$value));
    }};
    ($name:literal: $type:ty = $value:expr $(, unit = $unit:literal)?) => {{
        #[cfg_attr(target_os = "none", unsafe(link_section = ".data.tuning_studio"))]
        #[unsafe(export_name = $name)]
        static WATCH: $crate::Watch<<$type as $crate::Watchable>::Snapshot> =
            $crate::Watch::new()$(.unit($unit))?;
        WATCH.set(<$type as $crate::Watchable>::snapshot(&$value));
    }};
}

/// A value [`watch!`] can copy for the host.
///
/// Every `Copy` type is its own snapshot. For a struct that is not `Copy`, such
/// as one holding a driver handle, derive it:
///
/// ```
/// # struct CanDriver;
/// #[derive(tuning_studio_api::Watchable)]
/// struct Motor {
///     speed: f32,
///     #[watch(skip)]
///     driver: CanDriver,
/// }
///
/// fn control(motor: &Motor) {
///     tuning_studio_api::watch!("motor": Motor = *motor);
/// }
/// ```
///
/// The derive generates `MotorSnapshot`, a `Copy` struct with each kept field's
/// snapshot under the same name, so nested structs that derive it work too.
/// It does not support generic types or enums; implement the trait by hand for
/// those.
pub trait Watchable {
    /// What the host reads
    type Snapshot: Copy;
    fn snapshot(&self) -> Self::Snapshot;
}

impl<T: Copy> Watchable for T {
    type Snapshot = T;
    fn snapshot(&self) -> T {
        *self
    }
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
    /// Shown after the value; empty for none
    unit: &'static str,
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
            unit: "",
        }
    }

    /// The unit the host shows, such as `"rad/s"`.
    #[must_use]
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
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

/// Declare several [`Param`]s. Each option after the default calls the builder
/// method of that name, and attributes such as doc comments pass through. On
/// bare-metal targets the statics go in `.data.tuning_studio`, as [`watch!`]
/// values do. For a parameter in another section, declare a plain `static`
/// with its own `link_section`.
///
/// ```
/// tuning_studio_api::params! {
///     /// Target speed
///     pub static SETPOINT: f32 = 100.0, range = -300.0..=300.0, unit = "rad/s";
///     static KP: f32 = 0.02, range = 0.0..=1.0, persist = "drive.kp";
///     static ENABLE: bool = true;
/// }
/// ```
#[macro_export]
macro_rules! params {
    ($(
        $(#[$attr:meta])*
        $vis:vis static $name:ident: $type:ty = $default:expr $(, $option:ident = $value:expr)*;
    )*) => {$(
        $(#[$attr])*
        #[cfg_attr(target_os = "none", unsafe(link_section = ".data.tuning_studio"))]
        $vis static $name: $crate::Param<$type> = $crate::Param::new($default) $(.$option($value))*;
    )*};
}

/// A parameter the host can ask to change.
///
/// ```
/// use tuning_studio_api::Param;
///
/// static YAW_KP: Param<f32> = Param::new(1.2).range(0.0..=10.0).unit("1/s");
///
/// fn control_tick() {
///     let kp = YAW_KP.get();
///     // ...
/// }
/// ```
///
/// The host never writes the value the firmware runs with. It writes
/// `request`, then a new `request_id`. The next [`Param::get`] checks the
/// request, clamps it to the range, replaces `value`, and writes the result to
/// `reply` and the id to `reply_id`. The host waits for the reply before
/// sending another request.
#[repr(C)]
pub struct Param<T> {
    /// What the firmware runs with; only firmware writes it
    value: UnsafeCell<T>,
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
    persist: &'static str,
    unit: &'static str,
}

// SAFETY: firmware reads and writes `value` only inside a critical
// section; the host's fields are atomics.
unsafe impl<T: Value + Send> Sync for Param<T> {}

/// A restored parameter value is NaN and cannot be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidParamValue;

impl core::fmt::Display for InvalidParamValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("parameter value is NaN")
    }
}
impl core::error::Error for InvalidParamValue {}

impl<T: Value> Param<T> {
    /// A parameter that starts at `default`, with no range limit.
    #[must_use]
    pub const fn new(default: T) -> Self {
        Self {
            value: UnsafeCell::new(default),
            request: AtomicU32::new(0),
            request_id: AtomicU32::new(0),
            reply_id: AtomicU32::new(0),
            reply: AtomicU8::new(reply::NONE),
            default,
            min: T::MIN,
            max: T::MAX,
            persist: "",
            unit: "",
        }
    }

    /// Clamp requests to the range.
    #[must_use]
    pub const fn range(mut self, range: RangeInclusive<T>) -> Self {
        let (min, max) = (*range.start(), *range.end());
        self.min = min;
        self.max = max;
        self
    }

    /// Mark this parameter for firmware-managed flash storage under a stable key.
    ///
    /// Keys must be unique within the application's parameter store. Keep them
    /// unchanged across symbol renames. This hint does not perform flash I/O.
    ///
    /// # Panics
    /// Panics if `key` is empty.
    #[must_use]
    pub const fn persist(mut self, key: &'static str) -> Self {
        assert!(!key.is_empty(), "persistence key must not be empty");
        self.persist = key;
        self
    }

    /// The stable storage key, or `None` for a parameter kept only in RAM.
    pub const fn persistence_key(&self) -> Option<&'static str> {
        if self.persist.is_empty() {
            None
        } else {
            Some(self.persist)
        }
    }

    /// Restore a value loaded by the firmware's flash backend, clamped to the range.
    ///
    /// Load at boot before accepting host edits. Restoring does not acknowledge
    /// or discard a pending host request; the next `get` still applies it.
    /// For saving, write `last()` under `persistence_key()` after edits settle.
    /// Flash encoding, schema migration, wear, and save scheduling belong to
    /// the application. This method performs no flash I/O.
    ///
    /// # Errors
    /// Returns [`InvalidParamValue`] for a NaN, leaving the current value intact.
    pub fn restore(&self, value: T) -> Result<T, InvalidParamValue> {
        if value.partial_cmp(&value).is_none() {
            return Err(InvalidParamValue);
        }
        let this = escape(self);
        critical_section::with(|_| {
            let (value, _) = T::limit(value, this.min, this.max);
            // SAFETY: every firmware access to `value` holds this critical section.
            unsafe {
                *this.value.get() = value;
            }
            Ok(value)
        })
    }

    /// The unit the host shows, such as `"rad/s"`.
    #[must_use]
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = unit;
        self
    }

    /// Apply a pending request in full, clamped to the range, and return the value.
    /// Call it once per control tick, where a change is safe to take.
    pub fn get(&self) -> T {
        let this = escape(self);
        critical_section::with(|_| {
            // SAFETY: every firmware access to `value` holds this critical section.
            let value = unsafe { &mut *this.value.get() };
            let id = this.request_id.load(Ordering::Acquire);
            if id != this.reply_id.load(Ordering::Relaxed) {
                let reply = match T::from_request(this.request.load(Ordering::Relaxed)) {
                    Some(request) => {
                        let (limited, reply) = T::limit(request, this.min, this.max);
                        *value = limited;
                        reply
                    }
                    None => reply::REFUSED,
                };
                this.reply.store(reply, Ordering::Relaxed);
                this.reply_id.store(id, Ordering::Release);
            }
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

/// The types a [`Param`] or [`WatchMut`] can hold: at most 32 bits, so the host reads them
/// whole.
pub trait Value: Copy + PartialOrd + sealed::Sealed {
    #[doc(hidden)]
    const MIN: Self;
    #[doc(hidden)]
    const MAX: Self;
    #[doc(hidden)]
    fn from_request(bits: u32) -> Option<Self>;
    #[doc(hidden)]
    fn limit(request: Self, min: Self, max: Self) -> (Self, u8);
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
            fn from_request(bits: u32) -> Option<Self> {
                // The host zero-extends the value's bytes, so truncating recovers them.
                Some(bits as $type)
            }
            fn limit(request: Self, min: Self, max: Self) -> (Self, u8) {
                limit(request, min, max)
            }

        }
    )*};
}

integer!(i8, i16, i32, u8, u16, u32);

impl sealed::Sealed for f32 {}
impl Value for f32 {
    const MIN: Self = f32::NEG_INFINITY;
    const MAX: Self = f32::INFINITY;
    fn from_request(bits: u32) -> Option<Self> {
        Some(f32::from_bits(bits)).filter(|value| !value.is_nan())
    }
    fn limit(request: Self, min: Self, max: Self) -> (Self, u8) {
        limit(request, min, max)
    }
}

impl sealed::Sealed for bool {}
impl Value for bool {
    const MIN: Self = false;
    const MAX: Self = true;
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
    fn applies_requests_in_get_within_range() {
        let kp = Param::new(1.0f32).range(0.0..=10.0);
        assert_eq!(kp.get(), 1.0);
        assert_eq!(reply(&kp), (0, reply::NONE));

        let id = request(&kp, 2.0f32.to_bits());
        // Nothing changes until the firmware calls `get`.
        assert_eq!(kp.last(), 1.0);
        assert_eq!(kp.get(), 2.0);
        assert_eq!(reply(&kp), (id, reply::APPLIED));
        assert_eq!(kp.get(), 2.0);
        assert_eq!(kp.get(), 2.0);

        let id = request(&kp, 20.0f32.to_bits());
        kp.get();
        assert_eq!(reply(&kp), (id, reply::CLAMPED));
        let id = request(&kp, f32::NAN.to_bits());
        kp.get();
        assert_eq!(reply(&kp), (id, reply::REFUSED));
        // A refused request leaves the value alone.
        for _ in 0..100 {
            kp.get();
        }
        assert_eq!(kp.last(), 10.0);
        assert_eq!(kp.default_value(), 1.0);
    }

    #[test]
    fn integers_and_bools() {
        let offset = Param::new(0i16).range(-100..=100);
        request(&offset, (-50i16) as u16 as u32);
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
    fn persistence_is_explicit_and_restore_keeps_pending_host_edits() {
        let kp = Param::new(1.0f32).range(0.0..=10.0).persist("drive.kp");
        assert_eq!(kp.persistence_key(), Some("drive.kp"));
        assert_eq!(Param::new(0u8).persistence_key(), None);
        assert_eq!(kp.restore(20.0), Ok(10.0));
        assert_eq!(kp.get(), 10.0);
        assert_eq!(kp.restore(f32::NAN), Err(InvalidParamValue));
        assert_eq!(kp.last(), 10.0);
        let id = request(&kp, 3.0f32.to_bits());
        assert_eq!(kp.restore(5.0), Ok(5.0));
        assert_eq!(reply(&kp), (0, reply::NONE));
        assert_eq!(kp.get(), 3.0);
        assert_eq!(reply(&kp), (id, reply::APPLIED));
        assert_eq!(kp.default_value(), 1.0);
    }

    struct Driver;

    #[derive(Watchable)]
    struct Pid {
        integral: f32,
    }

    #[derive(Watchable)]
    struct Motor {
        speed: f32,
        pid: Pid,
        #[watch(skip)]
        _driver: Driver,
    }

    #[derive(Watchable)]
    struct Pair(u8, #[watch(skip)] Driver, i16);

    #[test]
    fn derives_snapshots_without_skipped_fields() {
        let motor = Motor {
            speed: 2.0,
            pid: Pid { integral: 0.5 },
            _driver: Driver,
        };
        watch!("test.motor": Motor = motor);
        watch!("test.speed": f32 = motor.speed, unit = "rad/s");
        let snapshot = motor.snapshot();
        assert_eq!((snapshot.speed, snapshot.pid.integral), (2.0, 0.5));
        assert_eq!(size_of::<MotorSnapshot>(), 8);
        let pair = Pair(1, Driver, -2).snapshot();
        assert_eq!((pair.0, pair.1), (1, -2));
        assert_eq!(f32::snapshot(&1.5), 1.5);
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
