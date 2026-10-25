# Type lab

STM32H723 firmware with one static for each value shape the CLI reads and
writes: every integer width, floats, special floats, `bool`, `char`, atomics,
C-like and data-carrying enums, `Option` with and without a niche, `Result`,
a union, structs, tuples, nested and large arrays, a `Cell`, pointers,
references, a `&str` and a slice.

It runs from the reset 64 MHz HSI clock with data in DTCM and the caches off, and
configures nothing besides SysTick. Every millisecond SysTick adds `live::STEP`
to `live::TICKS`, counts `live::MILLIS` and moves `live::WAVE` along a 1 s
triangle wave. Write `live::STEP` to check that the firmware sees writes.
`aggregates::TABLE` and the data behind `pointers::REFERENCE`, `TEXT` and `SLICE`
are in flash.

Build and flash it. Flashing replaces the board's firmware.

```sh
cd firmware/type-lab
cargo build --release
probe-rs download --chip STM32H723VGTx target/thumbv7em-none-eabihf/release/type-lab
probe-rs reset --chip STM32H723VGTx
```

Then, from the repository root:

```sh
tuning-studio read --chip STM32H723VGTx --elf firmware/type-lab/target/thumbv7em-none-eabihf/release/type-lab aggregates::ROBOT
```

The CLI's tests use a copy of the release ELF. After changing the lab, refresh
the copy and the expected output, and check the diff:

```sh
cp firmware/type-lab/target/thumbv7em-none-eabihf/release/type-lab crates/tuning-studio-cli/tests/fixtures/type_lab.elf
UPDATE_EXPECTED=1 cargo test -p tuning-studio-cli
```
