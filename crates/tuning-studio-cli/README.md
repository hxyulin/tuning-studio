# tuning-studio-cli

The `tuning-studio` command reads, writes and tunes variables in running
firmware through a [probe-rs](https://probe.rs) debug probe. It flashes and
runs firmware, prints its RTT and defmt logs, and serves GDB. Reads go through
the debug port while the core runs. Attaching and detaching halt the core for
about 20 ms.

```sh
cargo install --git https://github.com/hxyulin/tuning-studio tuning-studio-cli --locked
```

## Sessions

`run` and `attach` keep the probe open until Ctrl-C. While one runs, every
other command goes to it over a Unix socket, so they need no `--chip` or
`--elf` and do not attach again.

```sh
tuning-studio run --chip STM32H723VGTx target/thumbv7em-none-eabihf/release/app
```

`run` flashes the ELF, resets the target and prints its logs, so it also works
as a Cargo runner:

```toml
# .cargo/config.toml
[target.thumbv7em-none-eabihf]
runner = "tuning-studio run --chip STM32H723VGTx"
```

`attach` does the same for firmware that is already running, without flashing
or resetting. With `-i`, either one also takes commands from stdin; `help`
lists them. `--gdb` serves GDB on `127.0.0.1:1337`.

Without a session, a command attaches for its own run and needs `--chip`, and
`--elf` for symbol names. When several probes are connected, pick one with
`--probe VID:PID[:SERIAL]`.

## Commands

| Command | What it does |
| --- | --- |
| `read` | Read variables or addresses once |
| `watch` | Read them repeatedly until Ctrl-C, every `--interval` ms; 0 reads as fast as the probe can |
| `set` | Write one value, then read it back. A `Param` changes through the firmware |
| `values` | Print every `Param` and `watch!` value the firmware declares |
| `trace` | Record every write to up to four values for `--duration` ms, with its time and the function that wrote it |
| `tasks` | Profile every Embassy task for `--duration` ms |
| `halt`, `resume`, `step` | Run control |
| `break`, `delete` | Hardware breakpoints on a function or an address. Needs a session |
| `status` | Core status, and a session's breakpoints and watches |
| `flash`, `reset`, `erase` | Program, reset or erase the target |
| `list` | Connected debug probes |

`tuning-studio <command> --help` lists each command's options.

## Locations

`read`, `watch` and `set` take symbol paths from the ELF's DWARF:

```sh
tuning-studio read CONTROL                    # a whole struct
tuning-studio read CONTROL.yaw.target_rpm     # one field
tuning-studio read buffer[3] "app::STATE.0"   # an array element, a tuple field
tuning-studio read 0x24000070:f32             # an address with a type
tuning-studio read 0x20000038:Axis.gains.2    # an address with a DWARF type and a path
tuning-studio read "app::waves::POOL[task 0].future"
```

Values print with their type. Structs, tuples and arrays print one field per
line. Enums print their variant, pointers their address, and `&str` and
slices their contents.
`set` writes integers, floats, `bool`, `char` and C-like enum variants.

The CLI refuses a raw write into a `Param`'s or a `watch!` value's fields. Set
the `Param` itself, and the firmware applies the request.

## Values and parameters

Firmware that uses [`tuning-studio-api`](../tuning-studio-api) declares
`Param`s and `watch!` values. `values` lists them:

```text
tune_lab::SETPOINT: f32 = 100.0 rad/s (default 100.0, -300.0 to 300.0, step 1.0)
motor.speed: f32 = 91.16411 rad/s
```

`set` on a `Param` leaves a request and waits up to 500 ms for the firmware's
reply. It reports when the firmware clamped the value and fails when the
firmware refused it. While the firmware moves toward the request, the `Param`
shows its target.

A read of a `watch!` value that overlapped a write is retried, up to 10 times.

## Reading several values

Each transfer from the probe costs a USB round trip, about 1 ms. Before each
`read`, `values` and `watch` tick, the CLI reads every value it will show,
merging those less than 128 bytes apart in the same RAM region into one
transfer. Due session watches merge the same way. Raw addresses are read on
their own, so the bytes around a register are never touched.

On the [tune lab](../../firmware/tune-lab) at 10 MHz SWD, watching all four
`watch!` values and two `Param`s ran at 623 ticks per second, against 146 when
each was read alone. One value ran at about 860. `tuning-studio-api` puts its
statics in one section, so they merge.

## Traces

`watch` samples a value, so it misses changes between reads. `trace` records
every write instead, with the core's data trace, and does not halt the core:

```text
$ tuning-studio trace motor.error tune_lab::KP --duration 1000
     0.000 ms  motor.error: f32 = 0.0014877319 rad/s
     0.991 ms  motor.error: f32 = 0.0014877319 rad/s
...
motor.error: 1033 writes, 1009 per second, from SysTick 1033
tune_lab::KP: 1028 writes, 1009 per second, 1028 reads, from Param::get 1028
1.01 s at 65 MHz
```

Each DWT comparator matches one address, and the Cortex-M7 has four, so
`trace` takes up to four values. Each must be at most 4 bytes: an integer,
float, `bool` or C-like enum, or a `watch!` value or `Param` holding one. For a
struct, trace its fields, such as `motor.state.value.speed`. Times are in core
cycles, converted with the clock rate the CLI measures from the cycle counter.

The trace goes through the ETF, 2 KB of on-chip trace memory, which the CLI
drains through the debug port, so it needs no SWO pin. When writes arrive
faster than the probe drains them, the ETF fills, the ITM drops trace packets,
and `trace` reports how many times that happened. Each write takes up to 14
bytes of trace. On the [select lab](../../firmware/select-lab), a value written
faster than the probe drains it gave about 1,800 recorded writes per second at
the default 4 MHz SWD clock and 4,600 at `--speed 10000`.

`trace` has been tested on the STM32H723 only. It sets up the trace path
itself, since probe-rs's STM32H7 setup expects a second trace funnel the
H723 does not have.

## Tasks

`tasks` samples each Embassy task's future and the program counter while the
firmware runs. On the [select lab](../../firmware/select-lab):

```text
$ tuning-studio tasks --duration 2000
main: not spawned
control: running 26%, run-queued 1%
   99.1%  Suspend0 at src/main.rs:138
    0.9%  Suspend1 at src/main.rs:141
waves: running 44%, run-queued 75%
  100.0%  Suspend0 at src/main.rs:126
outside tasks: waker::wake 20%, Executor::run 6%, select_lab::settings 3%
1105 samples in 2.00 s, 552 per second
```

- `running` is the share of program counter samples inside the task's poll.
  It reads the DWT's `PCSR`. On a core without one, the line has no `running`.
- `run-queued` is the share of samples where the task waited to be polled.
- Each state line is an `.await` the task was parked on, with its source line.
- `outside tasks` names the functions the other samples landed in.

At about 550 samples per second, short tasks show up only as a share, not as
individual polls.

## Logs

`run` and `attach` print RTT up channels. A channel named `defmt` is decoded
with the ELF's defmt table, with timestamps and levels. Other channels print as
text. The session catches a HardFault, so firmware that panics through
`panic-probe` halts where it failed, after its message prints.
