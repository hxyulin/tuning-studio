# Tuning Studio desktop

An egui desktop app for Live Watch. It uses the same backend as the CLI and
VS Code extension, including `watch!` snapshots, `watch_mut!` edit mailboxes,
ranged parameters, structs, pointers, and supported Rust wrappers.

## Build and run

Build both executables from the repository root:

```sh
cargo build --locked -p tuning-studio-cli -p tuning-studio-desktop
cargo run --locked -p tuning-studio-desktop
```

The app finds `tuning-studio` beside its executable, then falls back to `PATH`.
You can override the CLI path under Connection. Select a probe, chip, and the
ELF matching the firmware already on the board, then Connect. Connection attaches
without flashing or resetting. If a matching CLI session already exists, the app
uses it and leaves it running when disconnected. Otherwise it starts an owned
CLI session and requests normal detach when disconnected or closed.

You can prefill the connection from a terminal:

```sh
target/debug/tuning-studio-desktop \
  --elf /absolute/path/to/firmware.elf \
  --chip STM32H723VGTx --probe VID:PID:SERIAL --connect
```

This first version supports macOS and Linux. The shared session backend uses
Unix sockets. Only macOS has been tested here. It requires exclusive probe access
unless connecting to a Tuning Studio session already serving that probe; a separate
probe-rs debugger cannot hold the same USB probe concurrently.

## Live Watch

- Declared `params!`, `watch!`, and `watch_mut!` values appear automatically.
- Expand structs and arrays with the row's disclosure button. Pointers and slices
  use the backend's bounded, lazy expansion. Existing wrapper decoding and write
  restrictions are preserved.
- Add symbols, task paths, or typed addresses using the input above the table.
  Suggestions appear after typing. Arrow keys select a suggestion; Enter accepts
  it, and a second Enter adds it. You can also add a full path directly.
- Click a writable value to edit it. Enter submits; Escape or leaving the field
  cancels. Polls do not replace your input. A failed write leaves it open to retry.
  Success closes the editor; the next backend read provides the displayed value.
- Read-only and unavailable values are dim. Hover for their reason, or click to
  open details. Read-only details include a copyable `watch_mut!` example; choose
  the actual source expression and call site yourself.
- Right-click a row to edit, Lock/Unlock, copy its raw value or full path, view
  details, or remove an added watch. A user lock is a local UI permission; it does
  not change firmware permissions or prevent other clients from writing.
- Select a row name and use F2 or Enter to edit it.
- The poll interval starts at 500 ms. Change it in the toolbar. Pause polling
  stops watch acquisition without halting the target. Halt and Resume control
  the target separately.
- Added watches, expansion, locks, connection settings, and window state persist
  between launches. The first version saves one watch configuration per app,
  rather than maintaining a separate configuration for every ELF/project.
- Each added watch is inspected separately so one failed path does not stop the
  remaining values. Results are snapshots taken at different times, not one
  simultaneous capture of all rows.

`watch_mut!` applies an edit at its next call; parameters apply requests at
`get()`. A halted core cannot consume either request until resumed. A timeout
message can therefore mean the request is still waiting, not rejected. Raw
scalar writes retain the backend's alignment and size limits and task-local
lifetime caveats.

The app's Session log includes firmware logs and backend errors. No extra RTT
producer is needed for raw watches or the existing API snapshots/mailboxes.
This initial app concentrates on watch/edit use. RTT sample capture and a native
plot panel are follow-on work; it does not add a new firmware streaming protocol.

## Validation

Model tests cover exact scalar copying and inherited snapshot read-only behavior.
A socket test checks output assembly, backend rejection, and truncated replies.
Hardware validation uses the STM32H723 tune-lab firmware on the Horco CMSIS-DAP
probe. See [the validation record](../../docs/investigations/native-app-validation.md)
for checks run on the board.
