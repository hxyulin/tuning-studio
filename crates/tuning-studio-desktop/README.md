# Tuning Studio desktop

An egui desktop app for Live Watch and plotting. It uses the same backend as the CLI and
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
## Plot

The single plot shares the existing polling samples with Live Watch. No extra
firmware code is needed for `watch!`, `watch_mut!`, parameter, or raw scalar plots.

- Use the plot's **+** menu, a watch's **Add to plot** context action, or drag a
  numeric watch name onto the plot. Up to 16 values share one time axis and y-scale.
- Click a legend name to hide/show it. Its samples keep accumulating while hidden.
  Click its small **×** to remove it. The **×** beside the title hides the panel;
  **Show plot** in the toolbar restores it without removing values.
- Click the title to rename it. Drag the divider between Live Watch and the plot
  to resize the watch panel.
- **Live** follows new samples and fits the visible values. **Freeze** holds the
  view while acquisition continues. Panning or zooming also freezes the view.
- Drag within the plot to pan. Drag the bottom axis horizontally or the left axis
  vertically to stretch that axis. Pinch or Ctrl-scroll to zoom; normal scrolling
  pans. Right-drag a box to zoom into it. Double-click resumes Live.
- Set the visible time window with the seconds control. **Fit** fits that window
  at the latest sample. **Clear** removes captured history without removing values.
- Plot title, membership, visibility, and time window persist across launches.
  Sample history stays in memory and starts empty on launch.

History is limited to ten minutes and 12,000 samples per value. Frozen views use
that same buffer, so old data can expire as acquisition continues. Pauses, failed
reads, non-finite values, and long sample gaps break the line. Plotted fields keep
being polled after their parent struct is collapsed or their manual watch removed.

The x-axis uses host receipt time in seconds since app launch. Values from one
poll can come from separate target reads. Numbers are plotted as `f64`; integers
above 2^53 can lose precision on the graph, while Live Watch copying stays exact.
This is low-rate polling rather than RTT capture. RTT streaming and sample export
remain follow-on work; this version adds no firmware streaming protocol.

## Validation

Model tests cover exact scalar copying and inherited snapshot read-only behavior.
A socket test checks output assembly, backend rejection, and truncated replies.
Hardware validation uses the STM32H723 tune-lab firmware on the Horco CMSIS-DAP
probe. See [the validation record](../../docs/investigations/native-app-validation.md)
for checks run on the board.
