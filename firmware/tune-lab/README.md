# Tune lab

STM32H723 firmware for checking `tuning-studio-api` on hardware. SysTick runs a
PI speed loop at 1 kHz against a simulated motor: speed moves toward
`300 * output` rad/s with a 50 ms time constant. The setpoint, gains, an
enable flag and an output limit are `Param`s. The error, output, speed and the
whole motor state are `watch!` values.

`motor.integral` is a mutable watch of the controller's existing integral field.
At the start of each tick, `watch_mut!` applies any edit and publishes the field.
The controller then updates it normally. This demonstrates editable state
without making it a parameter.

It runs from the reset 64 MHz HSI clock, with data in DTCM and the caches off,
and sleeps in `wfi` between ticks.

Build and run it. Flashing replaces the board's firmware.

```sh
cd firmware/tune-lab
cargo run --release
```

Then, from another terminal:

```sh
tuning-studio values
tuning-studio set tune_lab::SETPOINT 150    # ramps at 1 rad/s per tick
tuning-studio watch motor.speed tune_lab::SETPOINT
tuning-studio set tune_lab::KP 5            # clamped to 1.0
tuning-studio set tune_lab::KP.value 1      # refused: set the Param itself
tuning-studio set motor.integral 0.0        # assigned at the next tick
```

The CLI's tests use a copy of the release ELF. After changing the lab, refresh
it:

```sh
cp firmware/tune-lab/target/thumbv7em-none-eabihf/release/tune-lab crates/tuning-studio-cli/tests/fixtures/tune_lab.elf
```

On 2026-11-02, `watch_mut!` was checked on the STM32H723 with a Horco CMSIS-DAP
probe through the VS Code extension. After setting KI to zero, an edit to
`motor.integral` applied 0.25 and subsequent reads retained it. Changing the
setpoint from 100 to 180 rad/s produced the expected simulated-motor response
in the plot. Parameter clamping and rejection of direct writes to a read-only
watch or a mailbox backing field were also checked through the CLI.
