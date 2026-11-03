//! DWARF-only test image. Tests initialize pointers in host-side fake memory.
//! This fixture never allocates and is not flashed to a board.
#![no_std]
#![no_main]
extern crate alloc;
use alloc::{
    boxed::Box,
    rc::Rc,
    sync::{Arc, Weak},
};
use core::{
    alloc::{GlobalAlloc, Layout},
    cell::{Cell, RefCell, UnsafeCell},
    mem::MaybeUninit,
    pin::Pin,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicU32},
};
pub struct Shared<T>(UnsafeCell<T>);
// SAFETY: firmware never accesses these values. Tests mutate a host-side copy.
unsafe impl<T> Sync for Shared<T> {}
#[repr(C)]
pub struct Payload {
    pub count: u32,
    pub gain: f32,
}
#[repr(C)]
pub struct Node {
    pub value: u32,
    pub next: *mut Node,
}
const PAYLOAD: Payload = Payload {
    count: 7,
    gain: 0.5,
};
#[used]
pub static ATOMIC: AtomicU32 = AtomicU32::new(42);
#[used]
pub static FLAG: AtomicBool = AtomicBool::new(true);
#[used]
pub static CELL: Shared<Cell<u32>> = Shared(UnsafeCell::new(Cell::new(9)));
#[used]
pub static REFCELL: Shared<RefCell<Payload>> = Shared(UnsafeCell::new(RefCell::new(PAYLOAD)));
#[used]
pub static UNSAFE: Shared<UnsafeCell<Payload>> = Shared(UnsafeCell::new(UnsafeCell::new(PAYLOAD)));
#[used]
pub static MAYBE: Shared<MaybeUninit<Payload>> = Shared(UnsafeCell::new(MaybeUninit::new(PAYLOAD)));
#[used]
pub static DATA: Shared<Payload> = Shared(UnsafeCell::new(PAYLOAD));
#[used]
pub static BOXED: Shared<MaybeUninit<Box<Payload>>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static ARC: Shared<MaybeUninit<Arc<Payload>>> = Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static ARC_ATOMIC: Shared<MaybeUninit<Arc<AtomicU32>>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static RC: Shared<MaybeUninit<Rc<Payload>>> = Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static RC_REFCELL: Shared<MaybeUninit<Rc<RefCell<Payload>>>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static WEAK: Shared<MaybeUninit<Weak<Payload>>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static PINNED: Shared<MaybeUninit<Pin<Box<Payload>>>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static NONNULL: Shared<MaybeUninit<NonNull<Payload>>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static REFERENCE: Shared<MaybeUninit<&'static Payload>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static MUT_REFERENCE: Shared<MaybeUninit<&'static mut Payload>> =
    Shared(UnsafeCell::new(MaybeUninit::uninit()));
#[used]
pub static CYCLE: Shared<Node> = Shared(UnsafeCell::new(Node {
    value: 1,
    next: core::ptr::null_mut(),
}));
#[used]
pub static HEAP: Shared<[u32; 256]> = Shared(UnsafeCell::new([0; 256]));
struct NoAlloc;
// SAFETY: no allocations occur. Allocation fails with null; dealloc has no callers.
unsafe impl GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}
#[global_allocator]
static ALLOCATOR: NoAlloc = NoAlloc;
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
