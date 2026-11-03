# tuning-studio-api

Values for firmware that Tuning Studio reads and tunes over SWD:

- `watch!` copies a value into a static that the host reads. Only the firmware
  writes it.
- `Param` is a value the host asks to change. The firmware applies the request
  where it chooses, within a declared range, and replies.
- `watch_mut!` publishes a scalar local and applies host edits through a mutable
  reference. It leaves the local unchanged when no edit is pending.

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
it chose, then clamps it to the declared range. It applies the clamped value in full and writes back a reply. Each field has one
writer, so no write is lost or torn. The CLI also refuses raw writes into a
`Param`'s or a `watch!` value's fields.

## Use

```toml
[dependencies]
tuning-studio-api = { git = "https://github.com/hxyulin/tuning-studio" }
```

```rust
use tuning_studio_api::{params, watch};

params! {
    /// Target speed
    static SETPOINT: f32 = 100.0, range = -300.0..=300.0, unit = "rad/s";
    static KP: f32 = 0.02, range = 0.0..=1.0;
    static ENABLE: bool = true;
}

fn control(speed: f32) -> f32 {
    let error = SETPOINT.get() - speed;
    watch!("motor.error": f32 = error, unit = "rad/s");
    if ENABLE.get() { KP.get() * error } else { 0.0 }
}
```

Each option after the default calls the `Param` builder method of that name,
so a single parameter can also be written as
`static KP: Param<f32> = Param::new(0.02).range(0.0..=1.0);`.
Doc comments and other attributes pass through.

Call `get` once per control tick. Each call applies a pending request in full. A `Param` holds `f32`, `bool`, or an integer up to 32
bits, so the host can read it in one access.

`watch!` takes a name, the type, and the value. The name becomes the static's
symbol, so it must be unique, and a repeated name fails to link. A trailing
`unit = "rad/s"` gives the unit the host shows after a single value. A `watch!`
value can be any `Copy` type, including structs, or a type that derives
`Watchable`. Two counters around it let the host detect a read that overlapped
a write and retry it. Each use of the macro is one static, so a function called
from several tasks shares it, and the last call wins. The type cannot depend on
a generic parameter.

On bare-metal targets, `watch!` values and `params!` parameters go in the
`.data.tuning_studio` section. Linker scripts that place `.data.*`, such as
cortex-m-rt's, keep it with the rest of `.data`, and the values sit together.
The CLI reads values in the same RAM region less than 128 bytes apart in one
transfer, so watching many costs about as much as watching one. A trailing
`section = ".name"` puts a `watch!` value in that linker section instead,
which must be initialized at boot.

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

See [tune-lab](../../firmware/tune-lab) for a complete program, and
[select-lab](../../firmware/select-lab) for measurements of task-local reads
against `watch!` reads.

## Editable locals

Use `watch_mut!` when you want to inspect and edit an existing scalar without
moving it into a global parameter:

```rust
use tuning_studio_api::watch_mut;

fn tick(integral: &mut f32) {
    watch_mut!("pid.integral": f32 = integral);
    // Continue using the local normally.
}
```

For a field, pass `&mut controller.integral`. Each call consumes a pending edit,
assigns it through the mutable reference, publishes the resulting value, then
acknowledges the request. Without an edit, it only publishes. Firmware changes
afterward are published on the next call; an edit is applied once, not enforced
as an override. Put the call where the change should take effect, usually once
per control tick. The host never writes the local's address and the API never
stores its reference, so this works with ordinary task-owned state and needs no
Embassy feature or task-layout checks.

```sh
tuning-studio watch pid.integral
tuning-studio set pid.integral 0.0
```

`set` waits for the firmware's reply, with the same 500 ms timeout as parameters.
A timeout leaves the request pending. If the code stops calling the macro, the
snapshot remains at its last publication and requests wait. A pending request
can also be applied to a recreated local at the same call site. Acknowledgment
means that the edit was assigned there, not that later code preserved it.

Supported types are `f32`, `bool`, and signed or unsigned 8-, 16-, and 32-bit
integers. Invalid boolean and NaN requests are refused. There are no defaults,
ranges; use `Param` when you want those policies. Structs can
publish read-only snapshots with `watch!`; expose editable scalar fields with
separate `watch_mut!` calls.

As with `watch!`, names must be unique. Each macro call site owns one static
mailbox, shared by every call: if several objects or tasks use it, the next one
to call it consumes the edit. Use separate call sites and names for distinct
instances. `unit = "rad/s"` and `section = ".name"` work as for `watch!`.
You can also declare `static INTEGRAL: WatchMut<f32> = WatchMut::new();` and call
`INTEGRAL.sync(&mut controller.integral)` directly. The CLI rejects writes into
the snapshot and mailbox fields; set the mutable watch itself.

## Embassy task profiling

Enable the optional `embassy` feature to implement Embassy 0.10's seven executor
trace hooks. This also enables `embassy-executor/trace`; keep your application's
normal executor platform features. Only one trace hook provider can be linked.
Remove other `_embassy_trace_*` implementations when using this integration.

```toml
tuning-studio-api = { git = "https://github.com/hxyulin/tuning-studio", features = ["embassy"] }
embassy-executor = { version = "0.10", features = ["platform-cortex-m", "executor-thread"] }
```

Configure a clock once. On a Cortex-M with a DWT cycle counter, using your
existing core peripheral handle:

```rust,ignore
core.DCB.enable_trace();
cortex_m::peripheral::DWT::unlock();
core.DWT.enable_cycle_counter();
tuning_studio_api::embassy::init(
    cortex_m::peripheral::DWT::cycle_count,
    core::num::NonZeroU32::new(core_hz).expect("the core clock is nonzero"),
).expect("the profiler initializes once");
```

The API accepts any short `fn() -> u32` that reads a free-running wrapping clock.
It does not configure DWT itself. Keep the frequency fixed, and use a clock
shared by all executors. The critical-section provider must serialize their
hooks, including across cores if applicable.

From one task, publish periodically, such as every 100 ms:

```rust,ignore
tuning_studio_api::embassy::publish();
```

Then use the existing watch interface:

```sh
tuning-studio watch tuning_studio.tasks --interval 100
tuning-studio read tuning_studio.tasks
```

The snapshot contains up to 32 task instances, with opaque task and executor IDs,
an instance generation, alive/running/ready flags, completed poll and wake counts,
total elapsed poll ticks, and the longest poll. `sampled_at` records publication
time, and `tick_hz` gives the clock frequency. Completed instances remain until
their slot is reused; `untracked` counts spawns omitted when all slots are busy.
Counters wrap at 32 bits. Difference successive snapshots with wrapping
arithmetic, and only compare instances with matching IDs and generation.

Elapsed poll time includes interrupts and nested executor polls. It is not
exclusive CPU time; totals for nested executors can overlap. A poll must take
less than one full clock wrap. Polls that start before `init` still count but
do not contribute timing; `timed_polls` distinguishes them. Hooks update a fixed
table without allocations; `publish` copies it under a critical section and
publishes a consistent `Watch` snapshot. Measure the added latency on your board.
The CLI's `tasks` command continues to use its existing PC/future sampling;
hook counters are available through `values`, `read`, and `watch`.

### Task-local reads and edits

The integration does not require checkpoints in task code. Existing DWARF paths
can inspect task futures and edit exposed scalar locals. These operations remain
best effort while running: a task can change its future variant between resolving
a path and accessing memory. An address can then mean a different local, and a
write can be overwritten or corrupt task state. Trace hooks do not prevent this.

The CLI restricts edits through `[task N]` paths to naturally aligned 1-, 2-, or
4-byte scalar accesses. It rejects wider and unaligned edits before writing.
This avoids splitting the scalar across AP word writes; it does not validate the
local's lifetime or make read-modify-write operations atomic. These limits apply
even while halted. Raw address writes remain debugger operations and bypass
task-path checks. `watch!` copies remain read-only, and `Param` changes still go
through the firmware request protocol.

## Flash persistence

Opt in with a stable key. Parameters without a key stay transient:

```rust
use tuning_studio_api::{params, InvalidParamValue};
params! {
    static KP: f32 = 0.02, range = 0.0..=1.0, persist = "drive.kp";
}
fn restore_at_boot(saved_gain: f32) -> Result<(), InvalidParamValue> {
    KP.restore(saved_gain)?;
    Ok(())
}
// Your flash store saves KP.last() under KP.persistence_key().unwrap().
```

`persist` is metadata, not a flash operation. The CLI shows the key in parameter
reads and returns `persistence_key` in `inspect` JSON. Keys must be nonempty and
unique within your store. Keep a key unchanged when renaming its Rust symbol.
Choose a new key or migrate the record when changing a parameter's type or meaning.

The firmware loads records at boot and calls `restore`. Restored values use the
current range; NaNs are rejected without changing the value. A pending host edit
is left queued and still applies on the next `get`. Restoring does not change the
compiled default. Save `last()` after edits settle, on an explicit save action or
with debouncing. A record read from `last()` can become stale while an asynchronous
save runs; the application must track later changes if it displays a saved status.

Use your board's flash driver or a key-value store for encoding, schema versions,
checksums, power-loss recovery, and wear management. Save only the parameter
value and key, never the in-memory `Param` struct or its request counters. No
flash work happens inside `get`, a critical section, or an interrupt handler.

The API no longer ramps edits over successive `get` calls. Remove `max_step`
from older declarations. Keep slew limiting in the control code where it can use
elapsed time rather than the number of reads. Update the CLI with the firmware;
the current CLI also reads older parameter layouts.

## Not yet

- A built-in flash driver or a host Save to Flash command. Persistence keys and
  firmware restore hooks are available; the application owns storage.
- The same requests over USB serial, for builds without a probe attached.
