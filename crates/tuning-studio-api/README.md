# tuning-studio-api

Two primitives for firmware that Tuning Studio reads and tunes over SWD:

- `watch!` copies a value into a static that the host reads. Only the firmware
  writes it.
- `Param` is a value the host asks to change. The firmware applies the request
  where it chooses, within a declared range and step, and replies.

The crate is `no_std` and has no board, RTOS or transport dependency. It needs
a `critical-section` implementation, which `cortex-m`'s
`critical-section-single-core` feature or Embassy provides.

## Why

A debugger can read and write any static over SWD while the firmware runs.
For a control loop on a robot, that is not enough.

**Locals cannot be read reliably.** An Embassy task keeps the locals it holds
across an `.await` in its future, a compiler-generated enum. Each `.await` is a
variant, and the variants share the same bytes. A local's address therefore
means something only while the task is parked at that `.await`. The probe
reads the variant tag and the locals in separate transfers, and the task can
move on in between. While the task is being polled, its live values are in
registers and on the shared stack, and the bytes in the future are stale.
Locals that do not live across an `.await` never reach memory the host can
find at all.

`watch!` copies the value into a static at the point you choose. The static has
a fixed address, and it holds the last value written, whatever state the task
is in. The host reads it without halting the core.

**Nothing stops a write at the wrong time or to the wrong place.** A debugger
write lands whenever the probe sends it: halfway through the loop that uses
the value, into a variable the firmware overwrites on the next tick, or into a
field that holds a pointer or an enum tag. Nor does it know which values are
safe to change, or by how much. A gain typed as 50 instead of 0.5 reaches the
motor on the next tick.

`Param` never lets the host write the value the firmware runs with. The host
leaves a request. The firmware takes it in `Param::get`, at a point in its loop
it chose, then clamps it to the declared range. It moves toward the request by
at most `max_step` per call and writes back a reply. Each field has one
writer, so no write is lost or torn. The CLI also refuses raw writes into a
`Param`'s or a `watch!` value's fields.

## Use

```toml
[dependencies]
tuning-studio-api = { path = "../tuning-tools/crates/tuning-studio-api" }
```

```rust
use tuning_studio_api::{params, watch};

params! {
    /// Target speed
    static SETPOINT: f32 = 100.0, range = -300.0..=300.0, max_step = 1.0, unit = "rad/s";
    static KP: f32 = 0.02, range = 0.0..=1.0, max_step = 0.01;
    static ENABLE: bool = true;
}

fn control(speed: f32) -> f32 {
    let error = SETPOINT.get() - speed;
    watch!("motor.error": f32 = error);
    if ENABLE.get() { KP.get() * error } else { 0.0 }
}
```

Each option after the default calls the `Param` builder method of that name,
so a single parameter can also be written as
`static KP: Param<f32> = Param::new(0.02).range(0.0..=1.0).max_step(0.01);`.
Doc comments and other attributes pass through.

Call `get` once per control tick. Each call applies a pending request and
moves the value one step. A `Param` holds `f32`, `bool`, or an integer up to 32
bits, so the host can read it in one access.

`watch!` takes a name, the type, and the value. The name becomes the static's
symbol, so it must be unique, and a repeated name fails to link. A trailing
`section = ".name"` puts the static in that linker section; the section must be
zeroed at boot. A `watch!`
value can be any `Copy` type, including structs, or a type that derives
`Watchable`. Two counters around it let
the host detect a read that overlapped a write and retry it. Each use of the
macro is one static, so a function called from several tasks shares it, and
the last call wins. The type cannot depend on a generic parameter.

A struct that is not `Copy`, such as one holding a driver handle, derives
`Watchable`:

```rust
#[derive(Watchable)]
struct Motor {
    speed: f32,
    pi: Pi,              // derives Watchable too
    #[watch(skip)]
    driver: CanDriver,   // nothing to show
}

watch!("motor": Motor = *motor);
```

The derive generates `MotorSnapshot`, a `Copy` struct with the kept fields
under the same names, and `watch!` stores that. The host shows it as a normal
struct. A `Copy` type is already `Watchable`. Generic structs and enums that
are not `Copy` need a hand-written `impl Watchable`: pick a `Copy` snapshot
type and fill it in `snapshot`. Watching a reference or pointer would store an
address, and reads through it would be torn or dangling, so `watch!` always
stores a copy.

Then, with the firmware running:

```sh
tuning-studio values                    # every Param and watch! value
tuning-studio read motor.error
tuning-studio watch motor.error tune_lab::KP
tuning-studio set tune_lab::KP 0.05     # waits for the firmware's reply
```

`set` on a `Param` reports when the firmware clamps the request, and fails
when the firmware refuses it, such as a NaN. It also fails when the firmware
does not answer within 500 ms, for example while the core is halted.

## Layout

The host finds `Watch<T>` and `Param<T>` by type name in the DWARF, and reads
their fields by name. Keep debug information in the release profile
(`debug = 2`). A change to a field's name or meaning is a format change.

The crate passes each static's address through `core::hint::black_box` before
using it. Without that, LLVM saw every access to a `Param` at a constant
offset and, in an LTO build, split two of them into separate variables. The
DWARF then described storage that no longer existed.

## Not yet

- Saving parameters to flash, and restoring them at boot.
- The same requests over USB serial, for builds without a probe attached.
