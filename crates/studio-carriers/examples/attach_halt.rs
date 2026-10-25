//! How long probe-rs halts the core when a session opens and closes:
//! `cargo run --release -p tuning-studio-carriers --example attach_halt -- STM32H723VG`
//!
//! DWT CYCCNT stops while the core is halted, so the halt is the host time
//! between two counter reads minus the time the counter saw. Halts the target
//! briefly, many times.
use std::time::{Duration, Instant};

use probe_rs::architecture::arm::dp::DpAddress;
use probe_rs::architecture::arm::memory::ArmMemoryInterface;
use probe_rs::architecture::arm::{ApV2Address, FullyQualifiedApAddress};
use probe_rs::probe::list::Lister;
use probe_rs::{Permissions, Session};
use probe_rs_target::{ApAddress, CoreAccessOptions};

const DHCSR: u64 = 0xE000_EDF0;
const DEMCR: u64 = 0xE000_EDFC;
const DWT_CTRL: u64 = 0xE000_1000;
const DWT_CYCCNT: u64 = 0xE000_1004;
const DWT_LAR: u64 = 0xE000_1FB0;

fn memory_ap(session: &Session) -> FullyQualifiedApAddress {
    let CoreAccessOptions::Arm(options) = &session.target().cores[0].core_access_options else {
        panic!("not an Arm core");
    };
    let dp = options
        .targetsel
        .map_or(DpAddress::Default, DpAddress::Multidrop);
    match &options.ap {
        ApAddress::V1(ap) => FullyQualifiedApAddress::v1_with_dp(dp, *ap),
        ApAddress::V2(ap) => FullyQualifiedApAddress::v2_with_dp(dp, ApV2Address::new(*ap)),
    }
}

fn memory(session: &mut Session) -> Box<dyn ArmMemoryInterface + '_> {
    let ap = memory_ap(session);
    session
        .get_arm_interface()
        .unwrap()
        .memory_interface(&ap)
        .unwrap()
}

/// The counter, and the host time halfway through reading it
fn cycles(session: &mut Session) -> (u32, Instant) {
    let mut mem = memory(session);
    let before = Instant::now();
    let count = mem.read_word_32(DWT_CYCCNT).unwrap();
    (count, before + before.elapsed() / 2)
}

fn attach(chip: &str) -> Session {
    let mut probe = Lister::new().list_all()[0].open().expect("open probe");
    probe.set_speed(4000).unwrap();
    probe.attach(chip, Permissions::default()).expect("attach")
}

/// Milliseconds the counter missed between two reads
fn missed(hz: f64, a: (u32, Instant), b: (u32, Instant)) -> f64 {
    let host = (b.1 - a.1).as_secs_f64();
    let counted = b.0.wrapping_sub(a.0) as f64 / hz;
    (host - counted) * 1e3
}

fn report(label: &str, mut ms: Vec<f64>) {
    ms.sort_by(f64::total_cmp);
    println!(
        "{label:<34} min {:>6.2}  median {:>6.2}  max {:>6.2} ms  (n = {})",
        ms[0],
        ms[ms.len() / 2],
        ms[ms.len() - 1],
        ms.len()
    );
}

fn main() {
    let chip = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "STM32H723VG".into());
    let mut session = attach(&chip);
    {
        let mut mem = memory(&mut session);
        let dhcsr = mem.read_word_32(DHCSR).unwrap();
        assert!(dhcsr & (1 << 17) == 0, "the core is halted; run it first");
        let demcr = mem.read_word_32(DEMCR).unwrap();
        mem.write_word_32(DEMCR, demcr | 1 << 24).unwrap();
        mem.write_word_32(DWT_LAR, 0xC5AC_CE55).unwrap();
        let ctrl = mem.read_word_32(DWT_CTRL).unwrap();
        mem.write_word_32(DWT_CTRL, ctrl | 1).unwrap();
    }

    // Core clock from the counter over half a second without halting
    let a = cycles(&mut session);
    std::thread::sleep(Duration::from_millis(500));
    let b = cycles(&mut session);
    let hz = b.0.wrapping_sub(a.0) as f64 / (b.1 - a.1).as_secs_f64();
    println!("core clock {:.1} MHz", hz / 1e6);

    // The floor: two reads with nothing between them
    let floor = (0..20)
        .map(|_| {
            let a = cycles(&mut session);
            std::thread::sleep(Duration::from_millis(20));
            missed(hz, a, cycles(&mut session))
        })
        .collect();
    report("no halt (measurement floor)", floor);

    // What Session::new and Drop for Session each do
    let mut call = vec![];
    let clear = (0..20)
        .map(|_| {
            let a = cycles(&mut session);
            let start = Instant::now();
            session.clear_all_hw_breakpoints().unwrap();
            call.push(start.elapsed().as_secs_f64() * 1e3);
            missed(hz, a, cycles(&mut session))
        })
        .collect();
    report("clear_all_hw_breakpoints: halted", clear);
    report("clear_all_hw_breakpoints: call", call);

    // A disconnect and reconnect: Drop, then a new attach
    let mut reconnect = vec![];
    for _ in 0..10 {
        let a = cycles(&mut session);
        drop(session);
        session = attach(&chip);
        reconnect.push(missed(hz, a, cycles(&mut session)));
    }
    report("drop + attach: halted", reconnect);
}
