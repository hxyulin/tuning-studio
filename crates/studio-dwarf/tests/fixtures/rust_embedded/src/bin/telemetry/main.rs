//! Firmware-shaped fixture for the host's tuning-table decoder: one table with
//! a tunable of each shape and a watch of each kind, laid out by the same
//! `rm-telemetry` types the firmware uses.
#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod rm_telemetry;

pub mod tuning {
    use crate::rm_telemetry::{Table, Tunable, WatchBool, WatchF32, WatchI32, WatchU32};

    pub static PITCH_KP: Tunable =
        Tunable::new("gimbal.pitch.angle.kp", "1/s", 40.0, 0.0, 200.0, 0.2);
    pub static PITCH_KD: Tunable = Tunable::new("gimbal.pitch.angle.kd", "", 1.0, 0.0, 10.0, 0.01);
    pub static PITCH_ANGLE: WatchF32 = WatchF32::new("gimbal.pitch.angle_rad", "rad");
    pub static OFFSET: WatchI32 = WatchI32::new("gimbal.pitch.offset", "count");
    pub static TICKS: WatchU32 = WatchU32::new("robot.ticks", "");
    pub static ARMED: WatchBool = WatchBool::new("robot.armed", "");

    pub static TABLE: Table = Table::new(&[
        PITCH_KP.entry(),
        PITCH_KD.entry(),
        PITCH_ANGLE.entry(),
        OFFSET.entry(),
        TICKS.entry(),
        ARMED.entry(),
    ]);
}

#[no_mangle]
pub extern "C" fn reset() -> ! {
    let _ = tuning::TABLE.validate();
    let mut kp = tuning::PITCH_KP.default_value();
    let mut ticks = 0u32;
    loop {
        kp = tuning::PITCH_KP.apply(kp);
        let _ = tuning::PITCH_KD.apply(1.0);
        tuning::PITCH_ANGLE.publish(kp);
        tuning::OFFSET.publish(-(ticks as i32));
        tuning::TICKS.publish(ticks);
        tuning::ARMED.publish(ticks & 1 == 1);
        ticks = ticks.wrapping_add(1);
    }
}

#[link_section = ".vector_table.reset"]
#[used]
static RESET_VECTOR: extern "C" fn() -> ! = reset;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
