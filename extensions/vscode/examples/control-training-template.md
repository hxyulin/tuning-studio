# Control training template integration

The [training template](https://github.com/HKUAstar/control-training-template)
uses STM32H723VGTx, `thumbv7em-none-eabihf`, Embassy executor 0.10, and
`debug = 2` in both profiles. Its `Pid` is an empty exercise stub. The gain
examples below assume you have implemented `kp`, `ki`, and `kd` fields.

## Run from VS Code

Install the CLI from this checkout, which includes the extension's `inspect`
command:

```sh
cargo install --path crates/tuning-studio-cli --locked
```

Build and install the extension locally:

```sh
cd extensions/vscode
npm ci
npm run package
code --install-extension tuning-studio-0.1.0.vsix
```

Open your training repository in VS Code. Its existing `.cargo/config.toml`
already supplies the chip and target triple. Set the release arguments in
`.vscode/settings.json`:

```json
{
  "tuningStudio.cargoArgs": ["--release", "--locked"]
}
```

Click Run Firmware in the Tuning Studio sidebar. The extension runs Cargo with
a runner override, so Cargo gives Tuning Studio the exact ELF it just built.
The terminal prints RTT logs. Live Watch connects after the session answers.
You do not need a launch.json or a hardcoded ELF path for this project.

To use the same runner outside the extension, replace the existing runner in
`.cargo/config.toml`:

```toml
[target.thumbv7em-none-eabihf]
runner = "tuning-studio run --chip STM32H723VGTx"
# Keep the existing rustflags here.
```

Then `cargo run --release --locked` starts a session the extension can also use.
Stop a session started in another terminal there. Stop Session only terminates
the VS Code task the extension started. Ending a host session does not send a
stop command to the motor controller.

## Expose the values

Add the firmware API to the training repository:

```toml
[dependencies]
tuning-studio-api = { git = "https://github.com/hxyulin/tuning-studio" }
```

For an existing PID implementation, `watch_mut!` keeps the gain fields in the
controller. In `control_task`, synchronize them immediately before
`control.tick(&mut can)`, after `ticker.next().await`:

```rust,ignore
use tuning_studio_api::{watch, watch_mut};

// These gain fields must first be added to the template's Pid exercise.
watch_mut!("yaw.speed.kp": f32 = &mut control.yaw.speed_pid.kp);
watch_mut!("yaw.speed.ki": f32 = &mut control.yaw.speed_pid.ki);
watch_mut!("yaw.speed.kd": f32 = &mut control.yaw.speed_pid.kd);
watch_mut!("pitch.speed.kp": f32 = &mut control.pitch.speed_pid.kp);
watch_mut!("pitch.speed.ki": f32 = &mut control.pitch.speed_pid.ki);
watch_mut!("pitch.speed.kd": f32 = &mut control.pitch.speed_pid.kd);

control.tick(&mut can);

watch!("yaw.speed.measured": f32 = control.yaw.motor.speed_rpm, unit = "rpm");
watch!("yaw.speed.target": f32 = control.yaw.target_rpm, unit = "rpm");
watch!("yaw.angle.measured": f32 = control.yaw.motor.angle_rad, unit = "rad");
watch!("pitch.speed.measured": f32 = control.pitch.motor.speed_rpm, unit = "rpm");
watch!("pitch.speed.target": f32 = control.pitch.target_rpm, unit = "rpm");
```

Each mutable watch applies an edit once, on the next call, then publishes the
field. It retains no reference to the local. A later assignment by your code
can overwrite the edit. Keep a distinct macro call site for each controller;
one call site shared by yaw and pitch would share one mailbox.

This is the smallest retrofit. It has no range or rate policy. For bounded
gains that should eventually be saved to flash, declare `params!` and read them
into the corresponding gain fields before calculating the PID instead. Flash
persistence is not implemented yet. Keep controller history such as the
integral separate from the configured gain values.

## Watch, plot, and inspect tasks

Declared values appear in Live Watch automatically. Set Value sends a request
through the firmware mailbox and shows its reply. Use **+ Add variable** in
the Plot panel to choose numeric values. Plot time is the host's sample time,
not the control-loop timestamp.
Polling at 200 ms shows trends, not every iteration of a 1 kHz loop.

Sample Tasks captures the existing PC/future-state sampler for a short window.
It pauses the session's other reads during sampling. This first extension does
not turn the Embassy hook counters into a task timeline or exclusive CPU usage.
The API's optional [Embassy integration](../../../crates/tuning-studio-api/README.md)
can publish `tuning_studio.tasks`; that aggregate appears as a declared watch.

You can also add explicit DWARF paths in Live Watch, including task paths with
`[task 0]`. These remain best-effort debugger reads. Direct task-local writes
only support aligned 1-, 2-, or 4-byte scalar accesses. Use `watch_mut!` when you
want the task to apply an edit through its normal mutable reference.

## Fleet workspaces

Fleet's `xtask` generates a probe-rs launch configuration with the selected
board's chip and ELF. The extension imports those values without launching
probe-rs. Select that configuration and set:

```json
{
  "tuningStudio.buildTask": "xtask: build"
}
```

Run then executes the build task and starts `tuning-studio run` on its ELF.
Use a process or shell task that reports a successful exit code. A dependency
group without a process cannot establish that the build succeeded. Without a
build task, Run uses the ELF already on disk and says so in the Session view.
