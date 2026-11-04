# Live Watch, plotting, and existing tools

Date: 2026-11-04. Working research notes for the EGM brainstorm. No final architecture or Goldfish review yet.

## Problem and open boundary

We want to reduce the extension to tuning rather than own another debugger.
An existing probe-rs debug session should ideally provide the connection.
Short pauses may be acceptable for basic Live Watch. Plot capture may use a
separate acquisition path. Ozone may already satisfy enough of this need to
avoid building some features.

The unanswered boundary is whether the daily workflow must remain in VS Code
with the current CMSIS-DAP probe. That changes the value of a SEGGER-specific
solution. We also need the actual required plot sampling rate and a definition
of acceptable interruption: typical pause, worst observed pause, or a hard bound.

## How long does probe-rs pause?

We do not have a measurement for our probe and board. Do not interpret the
100 ms halt timeout as the actual duration. It is the time allowed for the halt
operation, not a bound covering the complete halt/read/resume sequence.

The source-inspected probe-rs 0.32 adapter only adds this halt for memory requests
when its last-known core status is Sleeping. Running cores do not pass through
that explicit sleep workaround. An idle async executor can enter sleep often;
we cannot assume this is rare in our workload. A sleeping CPU is not harmless
to halt: an interrupt that arrives during the halt may be delayed.

The interruption spans halt acknowledgement, memory access, and resume. It can
include several probe exchanges and host scheduling delays. A DAP request's full
wall time is different: it also includes VS Code transport and adapter queueing
outside the halted interval. End-to-end request latency alone cannot prove a
sub-millisecond interruption.

SEGGER publishes stop-mode examples: a four-byte read takes about 418 us with
J-Link ULTRA+ at 50 MHz JTAG, and about 718 us with J-Link PLUS at 15 MHz JTAG.
Its 256-byte examples are about 552 us and 1.02 ms respectively. These are typical
figures for SEGGER's stop-mode setups, not measurements or bounds for probe-rs,
CMSIS-DAP, SWD, or our STM32H723. See [SEGGER RTT modes](https://kb.segger.com/RTT).

A brief interruption may be acceptable, but its frequency matters too. For
example, a measured 0.5 ms pause repeated 100 times per second would accumulate
50 ms of halted time per second. This is illustrative arithmetic, not a target
measurement. One batched acquisition is different from one halt per displayed
field. A 1 ms maximum pause can also consume a whole period of a 1 kHz loop even
when average halted time is small.

A measurement should separate running and WFI/WFE cases, vary batch size and
probe speed, and report median, tail, and worst observed pauses. For true halt
duration use external timing, such as a suitable GPIO/timer waveform observed
with a logic analyzer, alongside host timestamps. Define whether debug freeze
stops the measurement timer. Instrumenting halt/read/resume in the adapter can
help locate delays but still does not establish a hard real-time bound.

Source: [probe-rs 0.32 request handling](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/debugger.rs).

## Existing tool approaches

| Tool | Mechanism and useful lesson | Boundary for us |
| --- | --- | --- |
| Keil | Watch/Memory can access running Cortex-M memory through CoreSight. Its Logic Analyzer uses a separate trace path. | Shows that watch polling and waveform capture need not use one mechanism. Does not establish that arbitrary Rust async locals stay valid. |
| Cortex-Debug | Live Watch uses a separate live-monitor GDB connection through supported GDB servers. It separates acquisition rate from UI refresh rate. It also supports RTT/SWO graphing. | Server-mediated sharing is different from two clients opening USB independently. Its approach is not proof that probe-rs accepts the same multi-client arrangement. |
| STM32CubeMonitor | Direct mode polls memory. Snapshot mode has firmware capture samples into a RAM buffer that the host drains. | Buffering preserves application-defined sampling times but costs firmware memory and execution time. |
| Ozone | Provides live watches, separate data sampling, timelines, Rust debugging, and docking. Native J-Link HSS samples through background memory access. | Could replace much of our general debugger UI. Our macros and Embassy awareness still need validation or integration. |
| J-Scope | HSS polls memory without application instrumentation; RTT captures application-provided data. RTT Plus combines host symbol selection with firmware sampling. | Particularly relevant if plotting is a separate tool. Requires SEGGER hardware for the documented J-Scope path. |

Sources: [Keil live memory example](https://www.keil.com/appnotes/files/apnt_332_v1.3.pdf),
[Cortex-Debug implementation](https://github.com/Marus/cortex-debug/blob/master/src/live-watch-monitor.ts),
[Cortex-Debug live GDB startup](https://github.com/Marus/cortex-debug/blob/master/src/gdb.ts),
[Cortex-Debug features](https://github.com/Marus/cortex-debug),
[CubeMonitor snapshot mode](https://wiki.st.com/stm32mcu/wiki/STM32CubeMonitor%3AHow_to_perform_an_acquisition_in_snapshot_mode),
[CubeMonitor acquisition rates](https://wiki.st.com/stm32mcu/wiki/STM32CubeMonitor%3AHow_to_modify_frequency_of_acquisition),
[Ozone manual](https://www.segger.com/downloads/jlink/UM08025_Ozone.pdf),
[J-Scope manual](https://kb.segger.com/J-Scope),
[J-Link HSS](https://kb.segger.com/HSS).

## What Ozone can replace

SEGGER documents live expression watches with per-item refresh rates, separate
data sampling and a timeline, standard debugger inspection and control, and Rust
source/state inspection. Ozone can therefore already cover much of the requested
watch/plot/debug UI. It deserves a real-ELF evaluation before more UI investment.

The manual describes 1–5 Hz live watches. High-speed acquisition uses a different
mechanism. HSS belongs to the J-Link SDK and uses background memory access; do
not extrapolate its speed to host-side DAP polling. The manual contains differing
headline maxima for data sampling, so this note does not promise a universal
sampling rate. Hardware, software version, variable count, and transport matter.

Ozone now also connects to third-party probes through a GDB server. SEGGER's
supported-feature page lists memory/register read/write and standard debugging.
This makes our existing probe plus a compatible server a candidate for basic
Ozone evaluation. It does not establish HSS, non-halting live reads, or native
J-Link feature parity through that server.

Rust support does not establish support for our specific optimized Embassy
future layouts, wrapper decoding, watch! publication protocol, watch_mut! mailboxes,
parameter ranges/replies, or generated persistence IDs. Ozone editing raw backing
memory is not equivalent to submitting an API edit and receiving its firmware
acknowledgement. These are the areas where a small Tuning Studio companion may
still be useful.

Ozone also exposes an automation socket that accepts console commands. It can
open an ELF and add watch expressions. This could make a thin setup helper
possible, but the documented socket is not a proven typed streaming API for our
plots. Do not build an integration around screen scraping.

Sources: [Ozone Rust announcement](https://www.segger.com/news/pr-240927-ozone-support-rust/),
[Ozone GDB support](https://www.segger.com/products/development-tools/ozone-j-link-debugger/technology/ozone-gdb-server-support/),
[Ozone automation socket](https://kb.segger.com/Connecting_to_Ozone%27s_Automation_Socket_Interface),
[Ozone licensing and third-party probe announcement](https://www.segger.com/news/pr-250909-ozone-simulators/).
The latter describes free evaluation and non-commercial use under its Friendly
License; commercial licensing depends on the probe/license combination.

## Candidate directions to test

1. **Slow companion Live Watch through probe-rs DAP.** Start read-only, use a
   bounded set of values, batch where possible, and measure sleep interruption.
   Keep editing disabled until access width and mailbox publication are settled.
   Counterargument: even slow polling can perturb an idle control loop, and a
   bridge to the Rust decoder is still work.
2. **Fix or extend upstream probe-rs.** Investigate whether this target actually
   needs the sleeping-core workaround and whether width-aware memory requests
   can be provided. Preserve low-power behavior on targets that need it.
   Counterargument: upstream support, older installed adapters, and target
   differences create a maintenance/version boundary. Never just remove the
   workaround globally because Cortex-M supports background access in general.
3. **Use firmware-buffered capture for plots.** Timestamp samples in the control
   task, collect related fields together, and drain a ring buffer in batches.
   RTT through the existing debugger or independent USB/UART are transport
   candidates. Include dropped-sample counts. Counterargument: firmware
   instrumentation and RAM use are unavoidable, and buffering cannot undo a
   debugger halt that delays the task producing samples.
4. **Use Ozone or J-Scope for general inspection/plots.** Evaluate our real ELF
   and a small set of PID fields before building replacements. Keep our API
   tools only for mailbox edits and Embassy-specific inspection if needed.
   Counterargument: separate app, probe/license constraints, and unknown support
   for our Rust-specific layouts.
5. **Keep standalone tuning sessions as an intentional mode.** The current CLI
   already supplies background reads with our decoder and aligned scalar writes.
   Counterargument: it does not satisfy simultaneous probe-rs debugging. It can
   be a fallback without becoming another extension-owned launch lifecycle.

No choice is final. The first experiments should answer whether the current
CMSIS-DAP setup can deliver acceptable DAP reads and whether Ozone already meets
the user-facing requirement on the actual firmware ELF.

## Requirements clarified after the initial investigation

The current board has no usable ITM/SWO path. Do not make either a requirement.
The user accepts a halt/edit/resume workflow for tuning. Some values should still
update live while firmware runs. Plotting is occasional and can use RTT instead
of the Live Watch polling path.

The narrowed working hypothesis is a companion to the existing probe-rs debug
session, with three separate behaviors:

- Live Watch reads a modest set of values through that session's memory requests.
  Measure the sleep workaround before promising a pause budget.
- Edits use the existing debugger's halt/resume controls. Raw scalar changes can
  be made while halted. API parameters and mutable watches still require their
  mailbox protocol; a queued edit is only applied when firmware resumes and
  reaches get() or watch_mut! again. Do not label a queued request as applied.
- Optional plots use timestamped firmware samples over a dedicated RTT channel,
  read by the existing probe owner. They are not a second process opening USB.
  Validate whether probe-rs exposes the required RTT data and framing to a
  companion extension before choosing an integration. Preserve normal log channels.

A halt makes multi-access writes easier to reason about, but does not establish
that an arbitrary task-local address is alive or will be used again. Existing
DWARF lifetime/state limitations remain. Reset, resume, and task-state changes
must invalidate address assumptions. No new DAP implementation, independent
probe owner, or extension-owned build/run lifecycle is proposed.

This is an anchored hypothesis awaiting user confirmation before Goldfish review,
not a completed design. The main open implementation checks are DAP read latency
and sleep impact, RTT channel access through the existing adapter, and reuse of
the Rust decoder without duplicating it in TypeScript.

## RTT as the tuning transport

Probe-rs 0.32's RTT channel reader reads ring-buffer offsets, copies available
bytes through target memory accesses, then publishes the consumed offset. There
is no halt/resume call in the channel read path. For a 32-bit target the host
publishes offsets with 32-bit accesses. Down channels provide host-to-firmware
bytes using the same ring-buffer pattern. See the [RTT module](https://docs.rs/probe-rs/0.32.0/probe_rs/rtt/index.html)
and [channel implementation](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs/src/rtt/channel.rs).

The earlier sleeping-core halt finding concerns the DAP memory-request handler.
It is not an inherent requirement of SWD or the RTT protocol. Background RAM
access can support RTT while Cortex-M code runs; low-power/debug access details
still depend on the target. Source inspection does not measure execution impact
on our board. Bus traffic, firmware copy/encoding cost, and blocking buffer modes
can affect timing without an explicit CPU halt.

An RTT-based Tuning Studio is therefore a separate candidate:

- Firmware publishes selected values and timestamped samples to an up channel.
- The extension receives data through the debugger's existing RTT owner.
- A down channel carries edit requests. Firmware validates and applies them at
  its update point and publishes acknowledgements. This needs a firmware receiver
  and a transport-aware API integration; it is not implemented by the existing
  memory mailboxes alone.
- Raw task locals and uninstrumented globals still use DWARF/debugger access.
  RTT can only report or change what firmware exposes and services.
- Breakpoints and halted inspection continue to use the normal debugger. While
  halted, no new samples or command handling occur; buffered output may drain.

Use a dedicated channel and explicit framing, nonblocking sample publication,
and dropped-sample counts. RTT is a byte stream, so a receive operation need not
return one complete sample. Use one host reader per channel, then fan data out
inside the host. Independent readers both updating the same consumed offset
would interfere. Upstream's channel reader even detects unexpected read-pointer
changes. DAP RTT event availability, binary framing, and down-channel access
through the probe-rs extension remain integration checks.

This candidate avoids requiring arbitrary live memory writes for normal tuning,
but adds firmware work and gives up transparent access to unexposed variables.
No implementation choice has been finalized.

## Can probe-rs own everything below the tuning UI?

For the debugger and probe connection, yes: use its existing VS Code debug
session for launch/attach, breakpoints, pause/resume, and RTT acquisition. The
probe-rs documentation describes RTT data and channel configuration DAP events.
A companion extension can observe those events without a second RTT reader.
See [probe-rs debugger integration](https://probe.rs/docs/tools/debugger/).

One concrete gap remains for commands: the 0.32 DAP request dispatcher handles
rttWindowOpened but has no RTT down-channel send request. The library supports
down channels, so this is an adapter exposure issue rather than a limitation of
RTT. A small upstream request would be preferable to recreating a debugger.
A library-owned session is a fallback but would retain our integration burden.
See [0.32 dispatcher](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/debugger.rs).

Tuning Studio still needs a versioned firmware data/command protocol, sample
framing, timestamps, schema interpretation, acknowledgements, and the tuning UI.
The existing RTT events' byte representation and binary round-trip integrity
must be validated before treating them as a raw binary transport.

There is no verified RTT throughput number for our Horco/STM32H723 setup. Do not
use SEGGER's headline figures as a CMSIS-DAP estimate. Throughput depends on SWD
speed, USB/probe firmware and packet batching, host polling, RTT buffer size,
other debugger/log traffic, encoding, and the DAP/extension delivery path.

A concrete initial benchmark workload is eight f32 values plus a u32 timestamp
and u32 sequence number per sample: 40 bytes. At 1 kHz this is 40,000 bytes/s
before framing, schema, transport encoding, and logs. That is a test target,
not an assertion that the connected hardware sustains it. Start with a lower
rate, increase until sequence gaps appear, and measure complete-path throughput,
latency, drops, firmware execution cost, and debugger responsiveness.

The first prototype can be receive-only on lab firmware with a dedicated RTT
channel. That validates Live Watch and plots without waiting for down-channel
support. Retain halt/edit/resume through the debugger during that experiment.
No vehicle firmware has been flashed for this investigation.

## Measured RTT trial

The test board has now been measured at 1, 4, and 10 MHz, using both direct RTT
and the existing probe-rs DAP event path. See the [measurement report](rtt-throughput.md)
for results and limits. Earlier statements above about absent measurements describe
the investigation before this trial. Down-channel edits remain untested.

## Measured low-rate Live Watch trial

At 2 Hz, the existing DAP sleeping-core workaround produced additional IRQ
service gaps above 1 ms. The existing AP background reader within a persistent
session retained the baseline while the same firmware slept. See the
[timing report](live-watch-timing.md) for the measured delays, controls, and limits.
Raw Live Watch and instrumented RTT capture can therefore remain separate paths.
