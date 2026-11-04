# Tuning Studio for VS Code

Watch and set firmware values, sample Embassy tasks, and plot
numbers live. The extension drives the `tuning-studio` CLI and never opens the
probe itself.

| View | Where | What it shows |
| --- | --- | --- |
| Session | Tuning Studio sidebar | Connection state, probe, and offline suggestions ELF |
| Live Watch | Tuning Studio sidebar | Declared `Param`, `watch_mut!` and `watch!` values, plus locations you add |
| Tasks | Tuning Studio sidebar | The last `tuning-studio tasks` sample |
| Plot | Tuning Studio panel | Plots with multiple values over host time |

The status bar shows the core state and opens the Session view.

See the [control training template integration](examples/control-training-template.md)
for the runner, firmware macros, and Fleet setup.

## Requirements

- `tuning-studio` with the `inspect` command, on `PATH` or set in
  `tuningStudio.executable`.
- macOS or Linux. Sessions use Unix sockets, so the extension reports Windows
  as unsupported.
- A running `tuning-studio run` or `attach` session. Stop probe-rs debugging
  before starting that session; shared access is not implemented yet.

## Connect to a tuning session

Build and start firmware from your terminal or existing project tooling:

```sh
# Flash, reset, and stream logs.
tuning-studio run --chip STM32H723VGTx path/to/firmware.elf

# Or attach to firmware already running, without flashing.
tuning-studio attach --chip STM32H723VGTx path/to/firmware.elf
```

Use your board's chip and ELF. Leave the terminal running and open Tuning Studio.
Live Watch discovers the session automatically. Use Ctrl+C in that terminal to
end it. The extension does not build, flash, start, stop, or restart firmware.
Existing VS Code build tasks and debugger controls remain independent.

For offline autocomplete, set `tuningStudio.elf`, or let the extension read
`coreConfigs[0].programBinary` from a probe-rs configuration in `.vscode/launch.json`.
`tuningStudio.launchConfiguration` selects its name; empty selects the first
`launch` configuration. Only the ELF is imported. No build tasks are executed.
Supported path variables are `${workspaceFolder}`, `${workspaceFolderBasename}`,
and `${env:NAME}`. Other variables produce an error; set `tuningStudio.elf`
explicitly in that case.

Run/Stop commands and the `runMode`, `chip`, `buildTask`, `cargoTarget`, and
`cargoArgs` extension settings have been removed. Existing values for these
settings can be deleted. The CLI's standalone commands still work.

## Probe

Session commands get `--probe` when `tuningStudio.probe` is set, and never get
`--chip` or `--elf`. Offline suggestions use `complete --elf <file>` and do not
attach to a probe. So a command reaches a running session or fails; it cannot
attach on its own. When sessions run for several probes, the Session view shows
the CLI's error. Select Probe lists the connected probes with `tuning-studio
list` and marks the ones that have a session.

## Live Watch

Watch suggestions require a CLI built from the same checkout as the extension.
After rebuilding the CLI, restart any existing run or attach session so it can
serve new commands. An older session falls back to suggestions from the selected
ELF. Pointer and active task-state suggestions still need a current live session.
Check `tuningStudio.executable` if the dropdown asks for a CLI update. Failed
suggestion requests are logged in the Tuning Studio Output channel.

Each poll runs `tuning-studio inspect` for the declared values, then
`tuning-studio inspect <locations...>` for the locations you added. Commands run
one at a time through one queue. If the whole added-watch call fails on a
location, Live Watch retries locations separately and shows each failure on its
own row. Recovery reads at most eight locations per poll, rotates through longer
lists, and stops starting retries after five seconds. Session and transport
failures are not retried. A failed read never contributes a stale plot sample.
The next poll starts `tuningStudio.pollInterval`
ms after the previous one started, or at once if it took longer.

Each value takes one row: the name on the left, the value on the right.
Structs and arrays have a disclosure arrow. Click the arrow or name to expand;
Left/Right arrows collapse or expand from the keyboard. Children show short field
names, with full paths in their hovers and Copy Path. Expansion survives polls
and webview recreation. Numeric descendants appear in Plot's **+ Add variable**
list even when their parent is collapsed.

Writable values use the number color. Read-only values are gray, and values
that could not be read show their error in gray italics. Hover a row for the
full value and the reason it is read-only or failed.

- Click a writable value, or focus its row and press Enter or F2, to edit it in
  place. Enter writes, Escape or leaving the field cancels. Polls keep updating
  the other rows without replacing your input, and a second Enter is ignored
  while a write is in flight.
- A failed write keeps the field open with the CLI's reason under it, so you
  can correct the input and press Enter again. After you leave the field, a
  small marker stays on the value until you edit it again.
- After a successful write the value stays dimmed until the next poll reads it
  back. A successful send is not proof that firmware applied a mailbox request.
- Right-click a row, click its `⋯` button, or press Shift+F10 for its actions:
  Edit Value, Show Reason or Show Error, Suggest watch_mut! in Source, Copy
  Value, Copy Path, and Remove Watch for locations you added. Copy Value copies
  the payload without its name, type, units, or parameter limits.
- Clicking a read-only or unavailable value, or pressing Enter/F2 on its row,
  opens a popover with its reason and any source suggestion. Escape or clicking
  outside dismisses it.
- Arrow keys, Home and End move between rows. The context menu can also show
  the reason inline; Escape hides it. Delete removes an added watch.
- Type in the field below the rows for symbol and field suggestions. The dropdown
  includes globals and typed Embassy task slots. Continue with `.`, `[` or `#`
  to explore a path. Up/Down selects a suggestion; Enter or Tab accepts it, and
  Enter again adds the watch. Escape dismisses suggestions without clearing your
  input. Manual symbol paths and `address:type` entries still work. Suggestions
  use the running session, or the selected ELF's static metadata before connecting.
  Live pointers and active task-state paths need a running session.
- Choose **Lock Value** or **Unlock Value** from a row's context menu to control
  editing. Locks persist per workspace and gray the value. Supported values,
  including runtime bookkeeping, stay editable by default. A user lock blocks
  writes to that path through this extension; it does not change firmware or other CLI clients.
  Unlock restores the backend's normal permissions and never bypasses a backend
  restriction.
- Type a symbol path or `address:type` and press Enter without selecting a
  suggestion to add it directly. The `+` button in the view title focuses this same field.
- Suggest watch_mut! in Source searches Rust files for the variable name.
  This is a search, not an exact DWARF source mapping. Select the mutable scalar
  expression in the task, then use **Insert watch_mut! for Selection** or the
  Rust refactor action. It inserts an editable snippet after the current line.
  Choose its scalar type and place the call before the task uses the value.
  Rebuild and flash before expecting the new declaration to appear.

A `watch!` struct's children come from the same counter-validated snapshot and
remain read-only. Supported fields of ordinary DWARF structs retain their own
write permissions and inline editors. Rust enums expose only the active payload.
Inspection includes up to 64 children per node, six levels, and 256 descendants
per root. Hover the parent for an omission note when a limit is reached.
Pointers and slices have an arrow but are read only when opened. A `*` row
contains a pointer's payload. Collapse a row to stop following it; a plotted
pointee keeps its pointer dependencies active. Null, invalid, changed pointers,
and cycles are reported in place. Opening a pointer does not pin its allocation
or pause firmware. Pointee bytes are live reads outside a `watch!` snapshot.
Each root has a 64 KiB pointee budget, and at most 64 pointer paths can be open.

Rust wrappers keep their useful type names. `Atomic` and `Cell` scalars show
values directly. Atomic writes require aligned 1-, 2-, or 4-byte accesses; the
debug probe does not supply Rust atomic memory-ordering semantics. `Box`,
`Pin<Box<_>>`, and `NonNull` expose their pointees. `Arc` and `Rc` show their
payloads rather than allocation headers; their hovers show strong/weak counts.
Shared payload fields are read-only unless an interior-mutable wrapper permits
an edit. `RefCell` shows its value and borrow state, and refuses writes while
borrowed; the host does not acquire a Rust borrow, so this remains best effort.
Refcount and borrow bookkeeping cannot be edited through typed paths. `Weak`
does not establish a live payload and reports that reason instead of following
it. `MaybeUninit` identifies its unknown initialization state. Unsupported
shared-pointer layouts fail with a reason instead of guessing an offset.

The wrapper fixtures cover sized payloads on Rust 1.99.1. Trait objects and
custom smart-pointer layouts are not decoded semantically. Shared references
and const pointers are read-only, except through supported interior-mutable
scalars. Older CLIs without structured children still show flat rows; use a
matching current CLI for pointer expansion.

Direct task-local edits require a currently resolvable local with an aligned
1-, 2-, or 4-byte scalar. The task can change state after resolution; writes
remain best effort. Locals optimized out of the binary, in inactive states,
or no longer retained by the future cannot always be resolved. `watch!`
snapshots are read-only. `watch_mut!` applies an edit at its next call and is
the preferred option for repeatable tuning of supported scalar locals.

The watch list and plot layout are saved per workspace.

## Tasks

Sample Tasks runs `tuning-studio tasks --duration <tuningStudio.tasksDuration>`
once. The session pauses its other work while it samples, so Live Watch pauses
too and the plot shows a gap. The view never samples by itself. The message
line shows compact sample totals, such as `83 samples · 200 ms · 414/s`.
Hover a task for the capture time. Show Tasks Output opens the
CLI's text for every sample. A state with a source line opens it on click.

## Plot

Drag a numeric value's name from Live Watch onto the plot to add it. The existing
**+ Add variable** list also works. Drag the **Variables** header to move the
floating list; resize it from the bottom-right corner. Its position and size
survive changes to the plotted values and webview recreation.

The panel shows one plot with up to 8 variables on a shared y-axis.
Choose **+ Add variable** below the variable list on the left. Click a variable
to hide or show its trace. Hidden variables keep collecting samples. Hover a
variable and click its × to remove it from the plot.

Drag the bottom time scale left/right to stretch the horizontal axis. Drag the
right value scale up/down to stretch the vertical axis. Scroll over the chart
for horizontal zoom, or Shift+scroll for vertical zoom. Double-click the chart
to reset both scales; double-click the time scale to reset only time. The time
scale also supports Left/Right arrow keys and Home to reset. Zoom survives
changes to plotted values and webview recreation.

Click the plot title to rename it, press Enter to save, or Escape to cancel.
The × in the top-right corner removes the plot and its variables. Add a
variable to start another plot. The title, variables, and visibility persist
per workspace. Older multi-plot layouts merge into this single plot, up to
8 variables.

- Time is host time when each `inspect` call returned, so it includes the USB
  round trip. It is not a firmware timestamp.
- A poll that read no number for a value, or failed, leaves a gap. So does every
  pause in polling.
- Each value keeps the last `tuningStudio.plotCapacity` samples. At the default
  200 ms interval, 6000 samples hold 20 minutes.
- Freeze stops the time axis so you can hover over past samples; recording
  continues.

The plot has no demo data. Without a session it stays empty.

## Settings

| Setting | Default | |
| --- | --- | --- |
| `tuningStudio.executable` | `tuning-studio` | Command name or absolute path |
| `tuningStudio.probe` | empty | `VID:PID` or `VID:PID:SERIAL` |
| `tuningStudio.launchConfiguration` | empty | probe-rs launch configuration name |
| `tuningStudio.elf` | empty | ELF for offline suggestions, overriding launch.json |
| `tuningStudio.pollInterval` | 200 | ms between polls |
| `tuningStudio.tasksDuration` | 200 | ms per tasks sample |
| `tuningStudio.plotWindow` | 30 | Seconds shown |
| `tuningStudio.plotCapacity` | 6000 | Samples per value |

## Limitations

- Only the first workspace folder is used.
- A location the CLI reports under a different path is matched by position, and
  only when the call returned one value per location.
- `inspect`'s optional `tasks` field is ignored; Tasks uses `tuning-studio tasks`.

## Earlier hardware validation

On 2026-11-02, the extension ran in the VS Code Extension Development Host on
macOS with a Horco CMSIS-DAP probe and an STM32H723 board. Run built and flashed
both `tune-lab` and `select-lab` from their Cargo configuration. Those Run
controls have since been removed; these results describe the earlier extension.

Live Watch discovered the API declarations. Set Value changed the simulated
motor's setpoint from 100 to 180 rad/s, and Plot showed its response. With KI
held at zero, editing `motor.integral` through `watch_mut!` applied 0.25 and
subsequent reads retained it. Supplemental CLI checks confirmed parameter
clamping and rejection of read-only watch and mailbox backing-field writes.

Sample Tasks captured `control` and `waves`, and clicking a state opened its
await statement in `select-lab/src/main.rs`. The optional hook profile was
readable as `tuning_studio.tasks`, with poll counters and no untracked tasks.
These checks do not measure hook latency or establish profiling accuracy on
other chips or executors.

## Development

```sh
npm install
npm run compile
npm test
```

Open this folder in VS Code and start **Extension on tune-lab** from Run and
Debug. It compiles the extension and opens [`firmware/tune-lab`](../../firmware/tune-lab)
in an Extension Development Host. `npm run package` builds a `.vsix` with
`@vscode/vsce`.

The tests cover the configuration and parser modules (`jsonc`,
`launchConfig`, `cliText`, `inspect`, `tasksOutput`, `series`,
`messages`), which do not import `vscode`.
