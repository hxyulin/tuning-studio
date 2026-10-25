//! Firmware-shaped Rust fixture for studio-dwarf. Statics live in nested
//! modules with mangled names, the way rm-embedded-rs declares them.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::num::NonZeroU32;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub mod telemetry {
    use super::*;

    pub static TX_FRAMES: AtomicU32 = AtomicU32::new(0);
    pub static LINK_UP: AtomicBool = AtomicBool::new(false);
    pub static mut SAMPLES: [u16; 8] = [0; 8];
}

pub mod control {
    use super::*;

    #[derive(Clone, Copy)]
    #[repr(u8)]
    pub enum Mode {
        Idle = 0,
        Running = 1,
        Fault = 7,
    }

    #[derive(Clone, Copy)]
    pub struct Pid {
        pub kp: f32,
        pub ki: f32,
        pub kd: f32,
        pub limit: Option<f32>,
    }

    /// Data-carrying enum: a `DW_TAG_variant_part` with an explicit tag.
    #[derive(Clone, Copy)]
    pub enum Command {
        Stop,
        Velocity(f32),
        Position { target: f32, max_speed: f32 },
    }

    pub struct Gimbal<const N: usize> {
        pub mode: Mode,
        pub pitch: Pid,
        pub yaw: Pid,
        pub command: Command,
        pub history: [f32; N],
        pub last_fault: Option<NonZeroU32>,
        pub offset: (i16, i16),
    }

    /// Shared cell the way embassy `Signal`/`StaticCell` wrap state.
    pub struct Shared<T>(pub UnsafeCell<T>);
    unsafe impl<T> Sync for Shared<T> {}

    pub static GIMBAL: Shared<Gimbal<4>> = Shared(UnsafeCell::new(Gimbal {
        mode: Mode::Idle,
        pitch: Pid { kp: 1.0, ki: 0.0, kd: 0.1, limit: Some(10.0) },
        yaw: Pid { kp: 2.0, ki: 0.0, kd: 0.2, limit: None },
        command: Command::Stop,
        history: [0.0; 4],
        last_fault: None,
        offset: (0, 0),
    }));

    pub mod nested {
        pub static mut TICKS: u64 = 0;
    }
}

#[no_mangle]
pub static mut C_COUNTER: u32 = 0;

#[inline(never)]
fn step(i: u32) {
    use control::*;
    let g = unsafe { &mut *GIMBAL.0.get() };
    g.mode = match i % 3 {
        0 => Mode::Idle,
        1 => Mode::Running,
        _ => Mode::Fault,
    };
    g.command = match i % 3 {
        0 => Command::Stop,
        1 => Command::Velocity(i as f32),
        _ => Command::Position { target: i as f32, max_speed: 1.0 },
    };
    g.history[(i % 4) as usize] = g.pitch.kp * i as f32;
    g.last_fault = NonZeroU32::new(i & 0xff);
    g.offset.0 = g.offset.0.wrapping_add(1);
    if let Some(l) = g.pitch.limit {
        g.pitch.ki = l;
    }
    telemetry::TX_FRAMES.fetch_add(1, Ordering::Relaxed);
    telemetry::LINK_UP.store(i & 1 == 1, Ordering::Relaxed);
    unsafe {
        let samples = &mut *core::ptr::addr_of_mut!(telemetry::SAMPLES);
        samples[(i % 8) as usize] = i as u16;
        nested::TICKS += 1;
        C_COUNTER = C_COUNTER.wrapping_add(1);
    }
}

#[no_mangle]
pub extern "C" fn reset() -> ! {
    let mut i = 0u32;
    loop {
        step(i);
        i = i.wrapping_add(1);
    }
}

#[link_section = ".vector_table.reset"]
#[used]
static RESET_VECTOR: extern "C" fn() -> ! = reset;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
