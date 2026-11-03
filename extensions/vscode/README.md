# Tuning Studio for VS Code

Run firmware, watch and set its values, sample its Embassy tasks, and plot
numbers live. The extension drives the `tuning-studio` CLI and never opens the
probe itself.

| View | Where | What it shows |
| --- | --- | --- |
| Session | Tuning Studio sidebar | Session state, probe, and how Run starts firmware |
| Live Watch | Tuning Studio sidebar | Declared `Param`, `watch_mut!` and `watch!` values, plus locations you add |
| Tasks | Tuning Studio sidebar | The last `tuning-studio tasks` sample |
| Plot | Tuning Studio panel | Plots with multiple values over host time |

The status bar item runs firmware when no session is running, and shows the
core state when one is.

See the [control training template integration](examples/control-training-template.md)
for the runner, firmware macros, and Fleet build task setup.

## Requirements

- `tuning-studio` with the `inspect` command, on `PATH` or set in
  `tuningStudio.executable`.
- macOS or Linux. Sessions use Unix sockets, so the extension reports Windows
  as unsupported.
- No probe-rs debug session at the same time. Run refuses to start while one
  is active, and warns when one starts during a session.

## Run

Run builds the firmware, flashes it with `tuning-studio run`, and waits until
`tuning-studio status` reaches the new session. Live Watch starts polling only
after that. Run uses one of two modes, chosen by `tuningStudio.runMode`:

| Mode | Command | Chosen by `auto` when |
| --- | --- | --- |
| `cargo` | `cargo run`, with `CARGO_TARGET_<TRIPLE>_RUNNER="tuning-studio run --chip <chip>"` | No ELF is known |
| `task` | The build task, then `tuning-studio run --chip <chip> <elf>` | An ELF is known from settings or launch.json |

Both run as VS Code tasks in a dedicated terminal, which prints the RTT and
defmt logs. `cargo` mode uses the `$rustc` problem matcher, so build errors
reach the Problems view. The runner override replaces any runner in
`.cargo/config.toml`, such as `probe-rs run`, for this run only.

Cargo configuration is read from Cargo home and from the workspace folder's
ancestors, with nearer files taking precedence. The legacy `.cargo/config`
filename wins over `config.toml` when both exist. Arrays merge as Cargo arrays do.
`CARGO_BUILD_TARGET` and target runner environment variables override file values.
The Session tooltips show the configuration file that supplied each value.

Each input comes from the first source that has it:

| Input | Settings | launch.json | Cargo |
| --- | --- | --- | --- |
| Chip | `tuningStudio.chip` | `chip` | `--chip` in `target.<triple>.runner` |
| ELF | `tuningStudio.elf` | `coreConfigs[0].programBinary` | |
| Build task | `tuningStudio.buildTask` | `preLaunchTask` | |
| Target triple | `tuningStudio.cargoTarget` | | `build.target` |

The launch.json configuration is the `probe-rs-debug` one named in
`tuningStudio.launchConfiguration`, or else the first `launch` request.
`programBinary` may use `${workspaceFolder}`, `${workspaceFolderBasename}` and
`${env:NAME}`; any other variable is reported instead of guessed. The Session
view shows each resolved input, and its tooltip names the source.

### A simple Cargo project

`.cargo/config.toml` with `build.target` and a runner that passes `--chip` is
enough, as in the [tune lab](../../firmware/tune-lab) or a project whose runner
is `probe-rs run --chip STM32H723VGTx`. Add `"tuningStudio.cargoArgs":
["--release"]` for release builds.

### A workspace with its own build tool

For a project like Fleet, where `cargo xtask build` writes the ELF and
launch.json names it, set the build task:

```jsonc
// .vscode/settings.json
{
  "tuningStudio.buildTask": "xtask: build",
  "tuningStudio.launchConfiguration": "Launch bringup-dm-mc02 (release)"
}
```

Without a build task, Run flashes the ELF already on disk, and the Session view
says so. A configured build task must report exit code zero before flashing.
Use a process or shell task rather than a task consisting only of dependencies.

### Stop

Stop terminates only the build or session that Run started. A session started
in a terminal shows as `external`; stop it there. Stop closes the task
terminal only while a build is running. For a running session, Stop sends an
owner-checked detach request. The CLI clears breakpoints, resumes a halted core,
and exits through its normal cleanup. A failed detach leaves the terminal open
and reports the reason. This needs a matching CLI with the `stop --owner` command.

## Probe

Every command gets `--probe` when `tuningStudio.probe` is set, and never gets
`--chip` or `--elf`. So a command reaches a running session or fails; it cannot
attach on its own. When sessions run for several probes, the Session view shows
the CLI's error. Select Probe lists the connected probes with `tuning-studio
list` and marks the ones that have a session.

## Live Watch

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
  Value, Copy Path, and Remove Watch for locations you added.
- Clicking a read-only or unavailable value, or pressing Enter/F2 on its row,
  opens a popover with its reason and any source suggestion. Escape or clicking
  outside dismisses it.
- Arrow keys, Home and End move between rows. The context menu can also show
  the reason inline; Escape hides it. Delete removes an added watch.
- Type a symbol path or `address:type` in the field below the rows and press
  Enter to add a watch. The `+` button in the view title asks for one instead.
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

The panel shows one plot with up to 8 variables on a shared y-axis.
Choose **+ Add variable** below the variable list on the left. Click a variable
to hide or show its trace. Hidden variables keep collecting samples. Hover a
variable and click its × to remove it from the plot.

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
| `tuningStudio.runMode` | `auto` | `auto`, `cargo` or `task` |
| `tuningStudio.launchConfiguration` | empty | probe-rs launch configuration name |
| `tuningStudio.chip`, `.elf`, `.buildTask`, `.cargoTarget` | empty | Override the imported values |
| `tuningStudio.cargoArgs` | `[]` | Extra `cargo run` arguments |
| `tuningStudio.pollInterval` | 200 | ms between polls |
| `tuningStudio.tasksDuration` | 200 | ms per tasks sample |
| `tuningStudio.plotWindow` | 30 | Seconds shown |
| `tuningStudio.plotCapacity` | 6000 | Samples per value |

## Limitations

- Only the first workspace folder is used.
- Cargo discovery reads target and runner keys. It does not evaluate `cfg(...)`
  runner conditions, config `include`, or `--config` overrides in `cargoArgs`.
- Cargo splits a runner from the environment on whitespace, so `cargo` mode
  refuses an executable path or probe selector with spaces. Use `task` mode.
- A location the CLI reports under a different path is matched by position, and
  only when the call returned one value per location.
- `inspect`'s optional `tasks` field is ignored; Tasks uses `tuning-studio tasks`.

## Hardware validation

On 2026-11-02, the extension ran in the VS Code Extension Development Host on
macOS with a Horco CMSIS-DAP probe and an STM32H723 board. Run built and flashed
both `tune-lab` and `select-lab` from their Cargo configuration.

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

The tests cover the configuration and parser modules (`jsonc`, `cargoConfig`,
`launchConfig`, `runPlan`, `cliText`, `inspect`, `tasksOutput`, `series`,
`messages`), which do not import `vscode`.
