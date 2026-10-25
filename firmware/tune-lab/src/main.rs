//! Tuning lab for the STM32H723: a simulated motor under PI speed control, with
//! the gains and setpoint as `Param`s and the loop's signals as `watch!` values.
//! It runs from the reset 64 MHz HSI clock with data in DTCM.
//!
//! SysTick runs the loop at 1 kHz. The motor is a first-order model: speed moves
//! toward `GAIN * output` with a 50 ms time constant.
#![no_std]
#![no_main]

use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::{entry, exception};
use tuning_studio_api::{params, watch};

params! {
    /// Target speed
    static SETPOINT: f32 = 100.0, range = -300.0..=300.0, max_step = 1.0, unit = "rad/s";
    static KP: f32 = 0.02, range = 0.0..=1.0, max_step = 0.01;
    static KI: f32 = 0.5, range = 0.0..=10.0, max_step = 0.1;
    /// Whether the controller drives the motor; off, the output is 0
    static ENABLE: bool = true;
    /// Largest output, in percent
    static OUTPUT_LIMIT: u8 = 100, range = 0..=100, max_step = 5, unit = "%";
}

/// Speed per unit of output, rad/s
const GAIN: f32 = 300.0;
const DT: f32 = 0.001;
const TAU: f32 = 0.05;

#[derive(Clone, Copy)]
struct Motor {
    speed: f32,
    integral: f32,
    ticks: u64,
}

static mut MOTOR: Motor = Motor {
    speed: 0.0,
    integral: 0.0,
    ticks: 0,
};

#[entry]
fn main() -> ! {
    let mut core = cortex_m::Peripherals::take().unwrap();
    core.SYST.set_clock_source(SystClkSource::Core);
    core.SYST.set_reload(64_000 - 1);
    core.SYST.clear_current();
    core.SYST.enable_interrupt();
    core.SYST.enable_counter();
    loop {
        cortex_m::asm::wfi();
    }
}

/// One step of the PI loop and the motor model.
fn control(motor: &mut Motor) {
    let setpoint = SETPOINT.get();
    let kp = KP.get();
    let ki = KI.get();
    let limit = f32::from(OUTPUT_LIMIT.get()) / 100.0;

    let error = setpoint - motor.speed;
    let output = if ENABLE.get() {
        let unclamped = kp * error + motor.integral;
        // Integrate only while the output is not saturated.
        if unclamped.abs() < limit {
            motor.integral += ki * error * DT * kp;
        }
        unclamped.clamp(-limit, limit)
    } else {
        motor.integral = 0.0;
        0.0
    };
    motor.speed += (GAIN * output - motor.speed) * DT / TAU;
    motor.ticks += 1;

    watch!("motor.error": f32 = error);
    watch!("motor.output": f32 = output);
    watch!("motor.speed": f32 = motor.speed);
    watch!("motor.state": Motor = *motor);
}

#[exception]
fn SysTick() {
    // SAFETY: only this handler touches MOTOR.
    control(unsafe { &mut *core::ptr::addr_of_mut!(MOTOR) });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
