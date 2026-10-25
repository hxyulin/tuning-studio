//! SWD type lab for the STM32H723: one static for each value shape the CLI reads
//! or writes. It runs from the reset 64 MHz HSI clock with data in DTCM and caches
//! off, and configures no peripherals besides SysTick.
//!
//! SysTick updates the statics in `live` at 1 kHz. Everything else changes only
//! when the host writes it.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering::Relaxed;
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};

/// Lets a `static` hold a value the host may change.
#[repr(transparent)]
pub struct Shared<T>(pub UnsafeCell<T>);

// SAFETY: only the host writes these values, between instructions of a running core.
unsafe impl<T> Sync for Shared<T> {}

pub mod scalars {
    #[used]
    pub static mut U8: u8 = 0xab;
    #[used]
    pub static mut U16: u16 = 0xbeef;
    #[used]
    pub static mut U32: u32 = 0xdead_beef;
    #[used]
    pub static mut U64: u64 = u64::MAX;
    #[used]
    pub static mut U128: u128 = u128::MAX;
    #[used]
    pub static mut USIZE: usize = 123_456;
    #[used]
    pub static mut I8: i8 = -8;
    #[used]
    pub static mut I16: i16 = -1600;
    #[used]
    pub static mut I32: i32 = -320_000;
    #[used]
    pub static mut I64: i64 = i64::MIN;
    #[used]
    pub static mut I128: i128 = i128::MIN;
    #[used]
    pub static mut ISIZE: isize = -123_456;
    #[used]
    pub static mut F32: f32 = 1.5;
    #[used]
    pub static mut F64: f64 = -2.25;
    #[used]
    pub static mut BOOL: bool = true;
    #[used]
    pub static mut CHAR: char = 'é';
    #[used]
    pub static mut NAN: f32 = f32::NAN;
    #[used]
    pub static mut INFINITY: f64 = f64::INFINITY;
    #[used]
    pub static mut NEG_ZERO: f32 = -0.0;
}

pub mod atomics {
    use core::ptr;
    use core::sync::atomic::{
        AtomicBool, AtomicI8, AtomicI16, AtomicI32, AtomicIsize, AtomicPtr, AtomicU8, AtomicU16,
        AtomicU32, AtomicUsize,
    };

    #[used]
    pub static U8: AtomicU8 = AtomicU8::new(7);
    #[used]
    pub static U16: AtomicU16 = AtomicU16::new(1600);
    #[used]
    pub static U32: AtomicU32 = AtomicU32::new(320_000);
    #[used]
    pub static USIZE: AtomicUsize = AtomicUsize::new(42);
    #[used]
    pub static I8: AtomicI8 = AtomicI8::new(-8);
    #[used]
    pub static I16: AtomicI16 = AtomicI16::new(-1600);
    #[used]
    pub static I32: AtomicI32 = AtomicI32::new(-320_000);
    #[used]
    pub static ISIZE: AtomicIsize = AtomicIsize::new(-42);
    #[used]
    pub static BOOL: AtomicBool = AtomicBool::new(true);
    #[used]
    pub static PTR: AtomicPtr<u32> = AtomicPtr::new(ptr::null_mut());
}

pub mod enums {
    use core::num::NonZeroU32;

    #[derive(Clone, Copy)]
    #[repr(u8)]
    pub enum Mode {
        Idle = 0,
        Running = 1,
        Fault = 7,
    }

    #[derive(Clone, Copy)]
    #[repr(i8)]
    pub enum Direction {
        Reverse = -1,
        Stop = 0,
        Forward = 1,
    }

    #[derive(Clone, Copy)]
    pub enum Command {
        Stop,
        Velocity(f32),
        Position { target: f32, limit: f32 },
    }

    #[used]
    pub static mut MODE: Mode = Mode::Running;
    #[used]
    pub static mut DIRECTION: Direction = Direction::Reverse;
    #[used]
    pub static mut COMMAND: Command = Command::Position {
        target: 1.25,
        limit: 4.0,
    };
    #[used]
    pub static mut STOP: Command = Command::Stop;
    #[used]
    pub static mut SOME: Option<f32> = Some(2.5);
    #[used]
    pub static mut NONE: Option<f32> = None;
    #[used]
    pub static mut NICHE: Option<NonZeroU32> = NonZeroU32::new(9);
    #[used]
    pub static mut RESULT: Result<u32, i16> = Err(-3);
}

pub mod aggregates {
    use super::Shared;
    use core::cell::{Cell, UnsafeCell};

    #[derive(Clone, Copy)]
    #[repr(C)]
    pub union Bits {
        pub word: u32,
        pub float: f32,
        pub bytes: [u8; 4],
    }

    #[derive(Clone, Copy)]
    pub struct Axis {
        pub position: f32,
        pub velocity: f32,
        pub gains: (f32, f32, f32),
    }

    pub struct Id(pub u32, pub i16);

    pub struct Robot<const N: usize> {
        pub axes: [Axis; 2],
        pub matrix: [[i16; 4]; 3],
        pub history: [f32; N],
        pub id: Id,
        pub cell: Cell<u32>,
    }

    const AXIS: Axis = Axis {
        position: 0.5,
        velocity: -0.25,
        gains: (10.0, 0.1, 0.01),
    };

    #[used]
    pub static mut ROBOT: Robot<4> = Robot {
        axes: [AXIS, AXIS],
        matrix: [[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]],
        history: [0.0, 0.25, 0.5, 0.75],
        id: Id(77, -7),
        cell: Cell::new(5),
    };
    #[used]
    pub static mut BITS: Bits = Bits { float: 1.0 };
    /// Filled with each element's index at startup.
    #[used]
    pub static mut LARGE: [u16; 300] = [0; 300];
    /// In flash, so writes must be refused.
    #[used]
    pub static TABLE: [u32; 4] = [1, 2, 3, 4];
    #[used]
    pub static SHARED: Shared<Axis> = Shared(UnsafeCell::new(AXIS));
}

pub mod pointers {
    use super::aggregates::{self, Axis};
    use super::scalars;
    use core::ptr;

    #[used]
    pub static mut TO_U32: *const u32 = &raw const scalars::U32;
    #[used]
    pub static mut TO_AXIS: *mut Axis = ptr::null_mut();
    #[used]
    pub static mut TO_POINTER: *const *const u32 = &raw const TO_U32;
    #[used]
    pub static mut TO_ARRAY: *const [u16; 300] = &raw const aggregates::LARGE;
    #[used]
    pub static mut NULL: *const u32 = ptr::null();
    #[used]
    pub static mut REFERENCE: &u32 = &aggregates::TABLE[2];
    #[used]
    pub static mut TEXT: &str = "type lab ✓";
    #[used]
    pub static mut SLICE: &[i16] = &[-1, 2, -3];
}

pub mod live {
    use core::sync::atomic::AtomicU32;

    /// Added to `TICKS` every millisecond; write it to check the firmware sees writes.
    #[used]
    pub static STEP: AtomicU32 = AtomicU32::new(1);
    #[used]
    pub static TICKS: AtomicU32 = AtomicU32::new(0);
    /// Milliseconds since startup, as a plain `u64` the core writes in two halves.
    #[used]
    pub static mut MILLIS: u64 = 0;
    /// Triangle wave from -1 to 1 with a 1 s period.
    #[used]
    pub static mut WAVE: f32 = 0.0;
}

#[entry]
fn main() -> ! {
    // SAFETY: SysTick, the only other code that touches these statics, starts below.
    unsafe {
        let large = (&raw mut aggregates::LARGE).cast::<u16>();
        for i in 0..300 {
            large.add(i).write(i as u16);
        }
        pointers::TO_AXIS = &raw mut aggregates::ROBOT.axes[1];
    }
    atomics::PTR.store(&raw mut scalars::U32, Relaxed);

    let mut core = cortex_m::Peripherals::take().unwrap();
    core.SYST.set_clock_source(SystClkSource::Core);
    core.SYST.set_reload(64_000 - 1);
    core.SYST.clear_current();
    core.SYST.enable_interrupt();
    core.SYST.enable_counter();
    // Spin rather than sleep: SWD access to D1 memory can fail while the core sleeps.
    loop {
        core::hint::spin_loop();
    }
}

#[exception]
fn SysTick() {
    let step = live::STEP.load(Relaxed);
    let ticks = live::TICKS.fetch_add(step, Relaxed).wrapping_add(step);
    // SAFETY: only this handler writes these statics.
    unsafe {
        live::MILLIS += 1;
        let phase = (ticks % 1000) as f32 / 500.0;
        live::WAVE = if phase < 1.0 {
            phase * 2.0 - 1.0
        } else {
            3.0 - phase * 2.0
        };
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
