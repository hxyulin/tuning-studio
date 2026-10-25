//! Copy of `rm-telemetry` from rm-embedded-rs (crates/rm-telemetry/src/lib.rs),
//! without its tests, so the fixture carries the table layout the host decodes.
//! Refresh it when the crate's format changes.
use core::sync::atomic::{AtomicU32, Ordering};

/// `Table::magic`: `RMTT` in little-endian byte order.
pub const TABLE_MAGIC: u32 = u32::from_le_bytes(*b"RMTT");
pub const TABLE_VERSION: u32 = 1;

/// How a cell's 32 bits are interpreted.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    F32 = 0,
    I32 = 1,
    U32 = 2,
    /// 0 or 1
    Bool = 3,
}

/// Who may change a value.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Published by the firmware; host writes are ignored.
    ReadOnly = 0,
    /// The owner applies host requests while running, within range and step.
    Live = 1,
}

/// One watchable or tunable value. Build it through [`Tunable`] or a watch
/// type, which fix the kind and access and give the firmware the matching
/// methods.
#[derive(Debug)]
pub struct Entry {
    /// FNV-1a of `name`: stable across builds while the name is.
    id: u32,
    kind: Kind,
    access: Access,
    /// Dotted path, for example `gimbal.pitch.encoder.angle.kp`.
    name: &'static str,
    /// Display unit, empty when dimensionless.
    unit: &'static str,
    /// Bits of the value the firmware was built with.
    default: u32,
    /// Inclusive range a request is clamped to. Infinite for watches.
    min: f32,
    max: f32,
    /// Largest change one [`Tunable::apply`] makes. Infinite for no limit.
    max_step: f32,
    /// Written by the host. The owner writes back only to correct a request it
    /// cannot take, so a host reading this sees what will be applied.
    requested: AtomicU32,
    /// Written by the owner: the value it is running with.
    applied: AtomicU32,
}

impl Entry {
    const fn new(
        name: &'static str,
        unit: &'static str,
        kind: Kind,
        access: Access,
        default: u32,
        range: (f32, f32, f32),
    ) -> Self {
        Self {
            id: fnv1a(name.as_bytes()),
            kind,
            access,
            name,
            unit,
            default,
            min: range.0,
            max: range.1,
            max_step: range.2,
            requested: AtomicU32::new(default),
            applied: AtomicU32::new(default),
        }
    }

    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn unit(&self) -> &'static str {
        self.unit
    }

    #[must_use]
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    /// The value the owner last applied or published, as raw bits.
    #[must_use]
    pub fn applied_bits(&self) -> u32 {
        self.applied.load(Ordering::Relaxed)
    }
}

const WATCH_RANGE: (f32, f32, f32) = (f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY);

/// An `f32` the host may change while the firmware runs.
#[derive(Debug)]
pub struct Tunable(Entry);

impl Tunable {
    /// `max_step` bounds the change per [`Tunable::apply`]: with one call per
    /// control tick, `(max - min) / max_step` ticks cross the whole range.
    ///
    /// # Panics
    ///
    /// At compile time, when used in a static, if the range is not finite and
    /// ordered, `default` lies outside it, or `max_step` is not positive.
    #[must_use]
    pub const fn new(
        name: &'static str,
        unit: &'static str,
        default: f32,
        min: f32,
        max: f32,
        max_step: f32,
    ) -> Self {
        assert!(min.is_finite() && max.is_finite() && min <= max);
        assert!(default >= min && default <= max);
        assert!(max_step > 0.0);
        Self(Entry::new(
            name,
            unit,
            Kind::F32,
            Access::Live,
            default.to_bits(),
            (min, max, max_step),
        ))
    }

    #[must_use]
    pub const fn entry(&self) -> &Entry {
        &self.0
    }

    #[must_use]
    pub const fn default_value(&self) -> f32 {
        f32::from_bits(self.0.default)
    }

    /// The value to run with this tick, given the one running now.
    ///
    /// A request outside the range is clamped, and one that is not a number
    /// is replaced by `current`; either correction is written back so the
    /// host sees it. The result moves from `current` toward the request by at
    /// most `max_step`, and is recorded as applied.
    pub fn apply(&self, current: f32) -> f32 {
        let e = &self.0;
        let raw = e.requested.load(Ordering::Relaxed);
        let wanted = f32::from_bits(raw);
        let target = if wanted.is_finite() {
            wanted.clamp(e.min, e.max)
        } else if current.is_finite() {
            current.clamp(e.min, e.max)
        } else {
            f32::from_bits(e.default)
        };
        if target.to_bits() != raw {
            // A newer host write wins over the correction of an older one.
            let _ = e.requested.compare_exchange(
                raw,
                target.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        let next = if current.is_finite() {
            current + (target - current).clamp(-e.max_step, e.max_step)
        } else {
            target
        };
        e.applied.store(next.to_bits(), Ordering::Relaxed);
        next
    }

    /// Ask for `value` from the firmware itself, as a host write would.
    pub fn request(&self, value: f32) {
        self.0.requested.store(value.to_bits(), Ordering::Relaxed);
    }

    #[must_use]
    pub fn requested(&self) -> f32 {
        f32::from_bits(self.0.requested.load(Ordering::Relaxed))
    }
}

macro_rules! watch {
    ($(#[$doc:meta])* $name:ident, $ty:ty, $kind:ident, $bits:expr) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $name(Entry);

        impl $name {
            #[must_use]
            pub const fn new(name: &'static str, unit: &'static str) -> Self {
                Self(Entry::new(name, unit, Kind::$kind, Access::ReadOnly, 0, WATCH_RANGE))
            }

            #[must_use]
            pub const fn entry(&self) -> &Entry {
                &self.0
            }

            pub fn publish(&self, value: $ty) {
                let bits: fn($ty) -> u32 = $bits;
                self.0.applied.store(bits(value), Ordering::Relaxed);
            }
        }
    };
}

watch!(
    /// An `f32` the firmware publishes for the host to sample.
    WatchF32, f32, F32, f32::to_bits
);
watch!(
    /// An `i32` the firmware publishes for the host to sample.
    WatchI32, i32, I32, |v| v.cast_unsigned()
);
watch!(
    /// A `u32` the firmware publishes for the host to sample.
    WatchU32, u32, U32, |v| v
);
watch!(
    /// A `bool` the firmware publishes for the host to sample.
    WatchBool, bool, Bool, u32::from
);

/// The firmware's list of values. One per binary; the host finds it by type.
#[derive(Debug)]
pub struct Table {
    magic: u32,
    version: u32,
    entries: &'static [&'static Entry],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TableError {
    /// Two names hash to one id, or one name is listed twice.
    DuplicateId {
        first: &'static str,
        second: &'static str,
    },
}

impl Table {
    #[must_use]
    pub const fn new(entries: &'static [&'static Entry]) -> Self {
        Self {
            magic: TABLE_MAGIC,
            version: TABLE_VERSION,
            entries,
        }
    }

    /// [`TABLE_MAGIC`] and [`TABLE_VERSION`] as built.
    #[must_use]
    pub const fn format(&self) -> (u32, u32) {
        (self.magic, self.version)
    }

    #[must_use]
    pub const fn entries(&self) -> &'static [&'static Entry] {
        self.entries
    }

    /// Check what a `const` cannot: ids are unique.
    ///
    /// Call it at boot as well as in a test. The call is also what keeps the
    /// table in the image: nothing else references it, and the linker drops
    /// unreferenced statics.
    pub fn validate(&self) -> Result<(), TableError> {
        let entries = core::hint::black_box(self).entries;
        for (i, a) in entries.iter().enumerate() {
            if let Some(b) = entries[..i].iter().find(|b| b.id == a.id) {
                return Err(TableError::DuplicateId {
                    first: b.name,
                    second: a.name,
                });
            }
        }
        Ok(())
    }
}

/// 32-bit FNV-1a.
#[must_use]
pub const fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

