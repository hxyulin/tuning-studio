# Rust wrapper fixture

This ELF supplies compiler-generated DWARF for atomics, `Cell`, `RefCell`,
`UnsafeCell`, `MaybeUninit`, `Box`, `Arc`, `Rc`, `Weak`, `Pin<Box<_>>`, `NonNull`,
shared/mutable references, and a cyclic node. It uses Rust 1.99.1 and the
`thumbv7em-none-eabihf` target. It is a test image, not runnable board firmware.

Rebuild from this directory:

```sh
cargo build --release
cp target/thumbv7em-none-eabihf/release/wrapper_fixture ../wrappers.elf
```

Tests load the ELF's sections into fake target memory. They initialize pointer
slots and allocation payloads there, using the real DWARF field offsets. The
fixture's allocator always returns null and is never called. No test flashes a
board or allocates through these pointers. The `MaybeUninit` slots let the ELF
retain the actual owning-pointer types without manufacturing invalid live Rust
values. These tests cover sized payloads; they do not establish support for
trait objects or alternate standard-library layouts.
