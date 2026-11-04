# A standalone Tuning Studio watch app

Date: 2026-11-04. Working hypothesis for the EGM brainstorm, not an approved design.

## What the debugger limitation actually is

The probe-rs library supports the background accesses already exercised by our
CLI on this board. Its 0.32 DAP handler explicitly halts a Sleeping core for
readMemory and resumes it afterward. That is an adapter policy, not a fundamental
restriction of VS Code, SWD, or the library. The inspected request path offers
no argument to opt out. We have not established a supported extension setting
that bypasses it.

Also distinguish synchronous host I/O from halting the target. A read may block
a worker thread while the MCU keeps running. A desktop app should keep probe I/O
off its UI thread; making its Rust function async does not itself change how
the target is accessed.

Source: [probe-rs 0.32 handler](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/debugger.rs).

## Smallest candidate app

One window provides probe selection, ELF selection, Connect/Disconnect,
Halt/Resume, and an expandable Live Watch table. Watch rows show the value,
allow inline edits where supported, and expose the existing failure reasons.
Polling initially runs at 2 Hz on a worker that owns the probe session.
The same worker serializes reads and commands; no UI operation directly holds
the probe. Persist only the selected ELF and watch list initially.

Reuse the current DWARF decoder, wrapper handling, parameter/mailbox semantics,
and aligned raw scalar writes. These already exist in the CLI implementation.
Do not duplicate that logic in a second frontend implementation.

The first prototype can bundle and own our existing CLI backend and use its
session interface. That already gives the tested AP memory path and avoids a
large shared-library extraction as a prerequisite. A later direct library API
should expose the same behavior when it removes repeated integration logic.
Use a persistent backend session; do not attach a new process for every read.

For a Rust app that owns target access directly, linking the probe-rs library
removes the need for a separately bundled probe-rs CLI. OpenOCD is not required
for the first supported probe/board and would introduce another process and
protocol boundary. A backend alternative should follow a concrete unsupported
probe/target need, not lead the prototype.

No DAP is required for this app's own halt/resume buttons or existing function/
address breakpoints. VS Code source breakpoint integration remains a separate
choice. Source-line breakpoint resolution, stepping and stack inspection should
not silently become requirements of a primitive watch app.

RTT acquisition and plots can be added after basic watch/edit use works. A
firmware sample producer remains necessary for instrumented RTT plots. Raw
background watch reads do not require that producer. Neither path should promise
transparent validity for inactive or optimized-out task locals.

## Where a desktop app helps

It removes VS Code view placement restrictions and the need to route UI commands
through an unrelated adapter's memory policy. The application can control
poll batching, request priorities, RTT decoding, and its own layout.

The hardware throughput does not improve merely because the UI is native.
The main target-access improvement is taking the tested background path. That
path could also support a VS Code frontend if exposed through the probe owner.
Avoid attributing the DAP sleep workaround to web UI rendering.

A desktop app also makes us own distribution, settings, window state, and
lifecycle. These can stay small if the first version remains a watch/tuning tool.

## UI toolkit considerations

- egui with eframe is the proposed first trial for a compact Rust tool. eframe
  supplies desktop integration and egui_plot supplies plotting widgets. This
  makes the primitive table/controls/plot combination feasible without a web UI.
  A deliberately styled interaction design still requires work.
- GPUI is another Rust-rendered option for a desktop interface with more custom
  layout and interaction work. Its upstream README explicitly warns of frequent
  breaking changes before 1.0, so pin dependencies for a trial.
- Tauri can reuse more existing HTML/CSS/JavaScript and keep a Rust backend, but
  retains a system webview and a frontend/backend IPC boundary. Its probe access
  can be just as capable as either Rust-rendered frontend.

Neither egui nor GPUI implies standard macOS/AppKit widgets; distinguish a native
executable with custom-rendered controls from a platform-widget toolkit.

Sources: [egui and eframe](https://github.com/emilk/egui),
[egui_plot](https://github.com/emilk/egui_plot),
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui),
[Tauri architecture](https://v2.tauri.app/concept/architecture/).

## Main boundary to resolve

An app that owns the USB probe cannot also assume an independent probe-rs VS Code
debugger owns it. Moving to a separate window does not solve connection sharing.
We must decide whether the primitive app intentionally owns the probe exclusively,
or whether simultaneous VS Code debugging is required. The existing backend's
GDB endpoint is a potential future integration route, not a tested solution here.

No implementation or toolkit migration has been started by this investigation.
