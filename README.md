<h1 align="center">Tuning Studio</h1>
<p align="center">Read, watch and tune running embedded firmware over SWD.</p>
<p align="center">
  <a href="rust-toolchain.toml"><img alt="Rust 1.99.1" src="https://img.shields.io/badge/Rust-1.99.1-dea584?logo=rust"></a>
  <a href="Cargo.toml"><img alt="Rust edition 2024" src="https://img.shields.io/badge/edition-2024-dea584"></a>
  <a href="#license"><img alt="MIT or Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue"></a>
</p>
<p align="center">
  <a href="crates/tuning-studio-cli">CLI</a> ·
  <a href="crates/tuning-studio-api">Firmware API</a> ·
  <a href="crates/studio-dwarf">DWARF library</a> ·
  <a href="firmware">Lab firmware</a>
</p>

`tuning-studio` reads variables out of a running microcontroller by name,
without halting it. It finds each symbol in the ELF's DWARF,
decodes structs, enums, arrays and pointers, and reads them through a
[probe-rs](https://probe.rs) debug probe while the core keeps running. The
`tuning-studio-api` crate adds two things a debugger cannot give firmware on
its own: values the host can always read whole, and parameters the firmware
changes itself, within limits it declares.

```text
$ tuning-studio values
tune_lab::KP: f32 = 0.02 (default 0.02, 0.0 to 1.0, step 0.01)
tune_lab::SETPOINT: f32 = 100.0 rad/s (default 100.0, -300.0 to 300.0, step 1.0)
motor.speed: f32 = 91.16411 rad/s
motor.state: MotorSnapshot
  speed: f32 = 91.17929
  ticks: u64 = 1123
  pi: PiSnapshot
    integral: f32 = 0.12814881
    saturated: bool = false

$ tuning-studio set tune_lab::SETPOINT 120
tune_lab::SETPOINT: f32 = 104.0 rad/s (target 120.0, default 100.0, -300.0 to 300.0, step 1.0)
```

| Task | Tuning Studio provides |
| --- | --- |
| Inspect state | `read` and `watch` on symbol paths such as `CONTROL.yaw.gains[2]`, or on raw addresses with a type, and `trace`, which records every write to a value |
| Tune a controller | `Param`s with a range and a step per tick, changed with `set`, which waits for the firmware's reply |
| Watch async code | `watch!` values that are never torn, and `tasks`, which profiles each Embassy task |
| Run firmware | `run` as a Cargo runner: flash, reset, then print RTT and defmt logs |
| Debug | `halt`, `resume`, `step`, hardware breakpoints, and a GDB server |
| Work from scripts | A `run` or `attach` session keeps the probe, and every other command reaches it over a local socket |

## Quickstart

<details>
<summary><strong>Install the CLI and read a running board</strong></summary>

Install [Rust](https://rustup.rs/), then the CLI from this repository:

```sh
cargo install --git https://github.com/hxyulin/tuning-studio tuning-studio-cli --locked
```

The installed command is `tuning-studio`. On Linux, probe-rs needs
[udev rules](https://probe.rs/docs/getting-started/probe-setup/) for your probe.

```sh
tuning-studio list                                    # connected probes
tuning-studio read --chip STM32H723VGTx --elf firmware.elf CONTROL.yaw
tuning-studio watch --chip STM32H723VGTx --elf firmware.elf --interval 10 imu.gyro
```

Use it as the Cargo runner in your firmware's `.cargo/config.toml`, so
`cargo run` flashes the board and keeps a session open:

```toml
[target.thumbv7em-none-eabihf]
runner = "tuning-studio run --chip STM32H723VGTx"
```

While that session runs, other terminals and scripts need no `--chip` or
`--elf`:

```sh
tuning-studio values
tuning-studio watch motor.speed
tuning-studio tasks --duration 2000
```

See the [CLI README](crates/tuning-studio-cli) for every command.

</details>

<details>
<summary><strong>Declare values and parameters in firmware</strong></summary>

```toml
[dependencies]
tuning-studio-api = { git = "https://github.com/hxyulin/tuning-studio" }
```

```rust
use tuning_studio_api::{params, watch};

params! {
    static SETPOINT: f32 = 100.0, range = -300.0..=300.0, max_step = 1.0, unit = "rad/s";
    static KP: f32 = 0.02, range = 0.0..=1.0, max_step = 0.01;
}

fn control(speed: f32) -> f32 {
    let error = SETPOINT.get() - speed;
    watch!("motor.error": f32 = error, unit = "rad/s");
    KP.get() * error
}
```

Keep debug information in the release profile (`debug = 2`). The
[API README](crates/tuning-studio-api) explains why task locals cannot be read
reliably and how `Param` keeps host writes safe.

</details>

## Crates

| Crate | What it is |
| --- | --- |
| [`tuning-studio-cli`](crates/tuning-studio-cli) | The `tuning-studio` command |
| [`tuning-studio-api`](crates/tuning-studio-api) | `no_std` firmware crate with `watch!`, `Param` and `params!` |
| [`tuning-studio-api-macros`](crates/tuning-studio-api-macros) | `#[derive(Watchable)]`, re-exported by `tuning-studio-api` |
| [`tuning-studio-dwarf`](crates/studio-dwarf) | ELF symbols, DWARF types and Embassy task layouts |

## Lab firmware

Each directory under [`firmware/`](firmware) is a standalone STM32H723 project
that checks one part of the CLI on hardware. Their READMEs record the
measurements.

| Lab | Checks |
| --- | --- |
| [type-lab](firmware/type-lab) | Reads and writes of every value shape: integers, floats, enums, `Option`, unions, pointers, slices |
| [log-lab](firmware/log-lab) | RTT text and defmt logs, and panics caught by `run` |
| [tune-lab](firmware/tune-lab) | `Param` and `watch!` on a PI loop driving a simulated motor |
| [select-lab](firmware/select-lab) | Reading async state under `select`, read rates, and the `tasks` profiler |

Tuning Studio has been tested on an STM32H723 with a CMSIS-DAP probe. Other
chips that probe-rs supports should work, but have not been tried.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

The CLI's tests read ELFs in `crates/tuning-studio-cli/tests/fixtures`, copied
from the labs. Each lab's README shows how to refresh its copy.

The `legacy` branch holds the earlier Tauri app and VS Code extension.

## License

Tuning Studio is available under either the [MIT license](LICENSE-MIT) or the
[Apache License 2.0](LICENSE-APACHE), at your option.
`tuning-studio-dwarf` was ported from `datavis-rs` and is MIT licensed only.
