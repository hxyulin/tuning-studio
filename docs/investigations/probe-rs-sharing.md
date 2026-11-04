# Sharing the probe-rs debugger connection

Date: 2026-11-04. Status: source investigation, not a reviewed design or a hardware-tested integration.

## Scope

Tuning Studio should focus on Live Watch, edits, plots, and firmware task information.
VS Code and the probe-rs extension should own debugging and its lifecycle.
We removed the extension's firmware Run/Stop commands, generated run tasks,
Cargo runner overrides, build discovery, process tracking, and termination fallbacks.
The CLI still provides standalone run and attach sessions. The extension observes
those sessions and does not control their lifetime. It still reads an ELF path
from settings or launch.json for offline autocomplete.

Shared probe access is not implemented by this cleanup. A tuning-studio session
and a probe-rs debug session must still run separately for now.

## What upstream provides

VS Code exposes `DebugSession.customRequest(command, args)`. An extension can use
an existing debug session's transport rather than opening another probe. The
session's resolved configuration is available too, including substituted paths.
A tracker can observe DAP messages and capabilities. It is not a probe handle.
See the [VS Code DebugSession API](https://code.visualstudio.com/api/references/vscode-api#DebugSession)
and [debug adapter tracker API](https://code.visualstudio.com/api/references/vscode-api#DebugAdapterTracker).

The [probe-rs extension activation code](https://github.com/probe-rs/vscode/blob/master/src/extension.ts)
registers its adapter and configuration providers. Its activation function does
not return a public shared-session API. We should use VS Code's debug-session
API rather than reach into its extension internals. This observation concerns
current upstream master, not a guarantee about every installed extension version.

Probe-rs 0.32.0 advertises `supportsReadMemoryRequest` and
`supportsWriteMemoryRequest`. It dispatches both requests through its existing
core handle. Its source does not impose a general halted-only restriction on
these requests. That makes the existing DAP session a candidate transport for
live memory reads. See the [0.32.0 request dispatcher and capabilities](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/debugger.rs).

The request shapes are standard DAP:

```ts
const reply = await session.customRequest('readMemory', {
  memoryReference: '0x24000000', offset: 0, count: 4,
});
// reply.data contains base64 bytes. Handle absent data and unreadableBytes.
```

This example illustrates the proposed transport. It is not wired into the extension.
See the [DAP memory request specification](https://microsoft.github.io/debug-adapter-protocol/specification#Requests_ReadMemory).

## Constraints that change the design

1. **Reads can disturb sleeping firmware.** Probe-rs 0.32.0 temporarily halts a
   sleeping core for memory requests and resumes it afterward. We cannot promise
   that this path never halts or changes sleep timing. See the
   [sleep handling before dispatch](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/debugger.rs).
2. **Writes do not preserve our scalar-access contract.** Its DAP memory handler
   delegates to `CoreHandle::write_memory`, which calls `core.write_8`.
   DAP does not specify the target bus access width. A four-byte request therefore
   cannot stand in for our explicit single 32-bit access. This also matters for
   firmware mailboxes: their payload and publication counters need the intended
   access ordering and widths. A halted target also cannot consume a mailbox
   request until resumed. See the
   [adapter handlers](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/debug_adapter/dap/adapter.rs)
   and [core memory helpers](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/core_data.rs).
3. **Our decoder is in Rust.** `inspect_expanded` already takes `&mut dyn Memory`,
   and autocomplete and wrapper decoding live beside it. Today those operations
   run inside a CLI session that owns the probe. We need a small transport bridge
   to let that decoder request bytes through the extension's DAP session. Moving
   all decoding into TypeScript would duplicate existing code.
4. **Memory reads do not replace task profiling.** DAP readMemory does not expose
   our PC sampler, DWT tracing, or Embassy profiling operations. Existing task
   samples can remain available through standalone CLI sessions. They should not
   silently pause or step the debugger in a shared-session prototype.
5. **Session selection and failures need explicit handling.** Bind to one
   probe-rs-debug session, not whichever session is currently focused. Clear
   cached memory on reads and session changes. Handle disconnects, unreadable
   ranges, older adapters, and multicore selection. Limit requests so polling
   does not monopolize the adapter or RTT processing.

These findings come from source inspection. We have not tested DAP polling,
throughput, sleep behavior, or writes on the connected board in this change.

## Smallest next experiment

Use an existing probe-rs debug session and read a known RAM scalar through
`customRequest('readMemory', ...)`. Check it while running, sleeping, halted,
and after resume. Check that RTT, breakpoints, Stop, and Restart still respond.
Use the resolved debug configuration to identify the ELF. Start with one core.

If that works, connect the existing Rust decoder's Memory interface to a narrow
request/reply bridge through the extension. Start with read-only Live Watch and
plots. Do not create another DAP server or another USB owner.

Before enabling live edits, establish an upstream operation that guarantees
aligned 8/16/32-bit target accesses, or another mechanism that preserves the
firmware mailbox publication rules. Standard writeMemory alone does not establish
that guarantee. This is the main open issue for a tuning-only companion extension.

## Alternatives weighed

- **Two independent probe connections:** does not share the existing probe-rs
  session or serialize access with its debugger. Do not use it.
- **A new Tuning Studio DAP:** would make us own breakpoint, stepping, stack, and
  lifecycle behavior. It expands the scope we are trying to reduce.
- **Wrap or fork the probe-rs adapter:** could add specialized requests, but
  brings upstream maintenance work. Investigate a small upstream change first.
- **Reuse probe-rs variables/evaluate instead of our decoder:** could give a
  smaller prototype, but would need separate checks for running async task
  locals, wrapper expansion, snapshots, and mailbox edits. Do not assume parity.
- **Shared remote server:** investigate its session ownership rules before
  treating a network connection as shared target ownership. It is not needed to
  test the existing VS Code DAP route.

Rust-analyzer integration is deferred. Target discovery is not required to test
probe sharing, and the extension no longer needs its own build/run discovery.
