#![no_std]
#![no_main]

use core::panic::PanicInfo;

// Simple struct
#[repr(C)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

// Struct with different types
#[repr(C)]
pub struct SensorData {
    pub id: u32,
    pub temperature: f32,
    pub pressure: f32,
    pub enabled: bool,
}

// Enum
#[repr(u32)]
pub enum State {
    Idle = 0,
    Running = 1,
    Error = 2,
}

// Generic struct (will be monomorphized)
#[repr(C)]
pub struct Buffer<T> {
    pub data: T,
    pub size: usize,
}

// Option type
#[repr(C)]
pub struct Device {
    pub id: u32,
    pub status: u8,
}

// Global static variables
#[no_mangle]
pub static mut COUNTER: u32 = 0;

#[no_mangle]
pub static mut SENSOR: SensorData = SensorData {
    id: 1,
    temperature: 25.0,
    pressure: 101.3,
    enabled: true,
};

#[no_mangle]
pub static mut POINT: Point = Point { x: 0, y: 0 };

#[no_mangle]
pub static mut STATE: State = State::Idle;

#[no_mangle]
pub static mut INT_BUFFER: Buffer<u32> = Buffer { data: 0, size: 0 };

#[no_mangle]
pub static mut FLOAT_BUFFER: Buffer<f32> = Buffer { data: 0.0, size: 0 };

// Array
#[no_mangle]
pub static mut SAMPLES: [u16; 8] = [0; 8];

// Main loop
#[no_mangle]
pub extern "C" fn main() -> ! {
    unsafe {
        loop {
            COUNTER = COUNTER.wrapping_add(1);
            SENSOR.temperature += 0.1;
            POINT.x += 1;
            POINT.y += 1;
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
