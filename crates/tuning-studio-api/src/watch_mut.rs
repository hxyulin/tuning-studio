//! Editable snapshots of scalar locals, synchronized by their owning code.

use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use crate::{Value, Watch, escape, reply};

/// Publish a scalar local and apply a pending host edit through its mutable reference.
///
/// ```
/// let mut integral = 0.0_f32;
/// tuning_studio_api::watch_mut!("pid.integral": f32 = &mut integral);
/// // Continue using `integral` normally. Call the macro again next tick.
/// ```
///
/// Each call applies at most one edit, then publishes the local's current value.
/// Without an edit it leaves the local unchanged. The reference is borrowed only
/// for this call and is never stored. See [`WatchMut::sync`] for request semantics.
///
/// Names must be unique, just as for [`crate::watch!`]. Each call site has one
/// static mailbox, even in a function called for multiple objects or tasks: the
/// next caller consumes a pending edit. Use separate call sites and names for
/// separate instances. The type cannot depend on a generic parameter.
///
/// `unit = "rad/s"` sets a display unit. `section = ".name"` selects an
/// initialized linker section; otherwise statics use `.data.tuning_studio` on
/// bare-metal targets. Supported types are [`Value`] scalars, not whole structs.
///
/// ```compile_fail
/// let mut ticks = 0_u64;
/// tuning_studio_api::watch_mut!("ticks": u64 = &mut ticks);
/// ```
#[macro_export]
macro_rules! watch_mut {
    ($name:literal: $type:ty = $value:expr $(, unit = $unit:literal)?, section = $section:literal) => {{
        #[unsafe(link_section = $section)]
        #[unsafe(export_name = $name)]
        static WATCH: $crate::WatchMut<$type> =
            $crate::WatchMut::new()$(.unit($unit))?;
        WATCH.sync($value);
    }};
    ($name:literal: $type:ty = $value:expr $(, unit = $unit:literal)?) => {{
        #[cfg_attr(target_os = "none", unsafe(link_section = ".data.tuning_studio"))]
        #[unsafe(export_name = $name)]
        static WATCH: $crate::WatchMut<$type> =
            $crate::WatchMut::new()$(.unit($unit))?;
        WATCH.sync($value);
    }};
}

/// A scalar snapshot with a host request mailbox, synchronized with a local.
///
/// Unlike [`crate::Param`], this has no default, range, or target. The local
/// remains the firmware's value. Only a new request changes it during [`Self::sync`].
/// The host discovers this type through DWARF, reads `snapshot` using the watch
/// counters, and uses the same request/reply fields as a parameter.
#[repr(C)]
pub struct WatchMut<T> {
    snapshot: Watch<T>,
    request: AtomicU32,
    request_id: AtomicU32,
    reply_id: AtomicU32,
    reply: AtomicU8,
}

impl<T> fmt::Debug for WatchMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchMut").finish_non_exhaustive()
    }
}

impl<T: Value> WatchMut<T> {
    /// Create an unpublished snapshot with no edit request.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            snapshot: Watch::new(),
            request: AtomicU32::new(0),
            request_id: AtomicU32::new(0),
            reply_id: AtomicU32::new(0),
            reply: AtomicU8::new(reply::NONE),
        }
    }

    /// Set the unit displayed beside the scalar.
    #[must_use]
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.snapshot = self.snapshot.unit(unit);
        self
    }

    /// Apply a pending edit and publish the local, without retaining its reference.
    ///
    /// Call at the point in your loop where edits should take effect. The host
    /// writes a 32-bit request followed by its ID. This call validates its scalar
    /// representation, assigns through `value`, publishes a consistent snapshot,
    /// then acknowledges the ID. An invalid boolean or NaN request is refused.
    /// Firmware can still publish its own NaN values.
    ///
    /// An acknowledged edit is applied once, not enforced on later calls. If
    /// firmware changes the local afterward, the next call publishes that value.
    /// Requests wait until the next call, including across a local's destruction
    /// and recreation at the same call site. They are not automatically cancelled.
    pub fn sync(&self, value: &mut T) {
        let this = escape(self);
        critical_section::with(|_| {
            let id = this.request_id.load(Ordering::Acquire);
            let pending = id != this.reply_id.load(Ordering::Relaxed);
            let response = if pending {
                match T::from_request(this.request.load(Ordering::Relaxed)) {
                    Some(request) => {
                        *value = request;
                        reply::APPLIED
                    }
                    None => reply::REFUSED,
                }
            } else {
                reply::NONE
            };
            this.snapshot.set(*value);
            if pending {
                this.reply.store(response, Ordering::Relaxed);
                this.reply_id.store(id, Ordering::Release);
            }
        });
    }
}

impl<T: Value> Default for WatchMut<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<T>(watch: &WatchMut<T>, id: u32, bits: u32) {
        watch.request.store(bits, Ordering::Relaxed);
        watch.request_id.store(id, Ordering::Release);
    }

    fn published<T: Value>(watch: &WatchMut<T>) -> T {
        assert_ne!(watch.snapshot.start.load(Ordering::Relaxed), 0);
        assert_eq!(
            watch.snapshot.start.load(Ordering::Relaxed),
            watch.snapshot.end.load(Ordering::Relaxed)
        );
        // SAFETY: this test owns the watch and has published an initialized value.
        unsafe { (*watch.snapshot.value.get()).assume_init() }
    }

    #[test]
    fn follows_firmware_until_an_edit_is_applied_once() {
        let watch = WatchMut::new();
        let mut local = 3.0_f32;
        watch.sync(&mut local);
        assert_eq!(published(&watch), 3.0);
        local = 4.0;
        watch.sync(&mut local);
        assert_eq!(published(&watch), 4.0);
        request(&watch, 1, 8.0_f32.to_bits());
        assert_eq!(local, 4.0, "the request cannot touch the local before sync");
        watch.sync(&mut local);
        assert_eq!(local, 8.0);
        assert_eq!(published(&watch), 8.0);
        assert_eq!(watch.reply_id.load(Ordering::Acquire), 1);
        assert_eq!(watch.reply.load(Ordering::Relaxed), reply::APPLIED);
        local = 9.0;
        watch.sync(&mut local);
        assert_eq!(
            local, 9.0,
            "an acknowledged edit must not become an override"
        );
        assert_eq!(published(&watch), 9.0);
    }

    #[test]
    fn refuses_invalid_requests_without_replacing_the_local() {
        let watch = WatchMut::new();
        let mut local = true;
        request(&watch, 1, 2);
        watch.sync(&mut local);
        assert!(local);
        assert!(published(&watch));
        assert_eq!(watch.reply_id.load(Ordering::Acquire), 1);
        assert_eq!(watch.reply.load(Ordering::Relaxed), reply::REFUSED);

        let watch = WatchMut::new();
        let mut local = 5.0_f32;
        request(&watch, 1, f32::NAN.to_bits());
        watch.sync(&mut local);
        assert_eq!(local, 5.0);
        assert_eq!(watch.reply.load(Ordering::Relaxed), reply::REFUSED);
    }

    #[test]
    fn requests_can_precede_first_publication_and_ids_can_wrap() {
        let watch = WatchMut::new();
        let mut local = 0_i16;
        request(&watch, u32::MAX, (-12_i16) as u16 as u32);
        watch.sync(&mut local);
        assert_eq!(local, -12);
        request(&watch, 0, 42);
        watch.sync(&mut local);
        assert_eq!(local, 42);
        assert_eq!(watch.reply_id.load(Ordering::Acquire), 0);
    }

    #[test]
    fn macro_borrows_a_field_once_and_publishes_it() {
        let mut state = (3_u32,);
        let mut calls = 0;
        crate::watch_mut!("test.mutable-field": u32 = {
            calls += 1;
            &mut state.0
        }, unit = "ticks");
        state.0 += 1;
        assert_eq!(calls, 1);
        assert_eq!(state.0, 4);
    }
}
