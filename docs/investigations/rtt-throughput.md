# RTT throughput on the connected Horco probe

Measured 2026-11-04. The user authorized replacing the test board's firmware.
At the end of that trial the board ran the dedicated RTT benchmark. It was later
replaced by the [Live Watch timing fixture](live-watch-timing.md).
All benchmark/debug-adapter connections were closed afterward. A final
`tuning-studio status` check reported `running`.

## Results

All rates below are decimal kB/s of original binary RTT data, including the
benchmark's packet headers. They exclude DAP's textual expansion and JSON framing.
Each row contains one approximately ten-second capture after warmup, per path.
These short captures establish working rates for this setup, not long-term limits.

| SWD clock | Intended role | Direct probe-rs RTT | Through probe-rs DAP events |
| --- | --- | ---: | ---: |
| 1 MHz | Conservative | 33.1 kB/s | 32.6 kB/s |
| 4 MHz | Normal | 90.6 kB/s | 87.4 kB/s |
| 10 MHz | Fast | 189.8 kB/s | 191.4 kB/s |

The direct probe API accepted each requested clock. No oscilloscope measurement
of actual SWCLK frequency was made. DAP was configured with the same requested
clocks. Small differences between paths should not be interpreted as a speed
advantage: these were separate short runs, without repeated-trial statistics.

All six captures had zero detected corrupt packets, zero sequence gaps, and zero
framing resynchronizations. Across them, 24,617 complete packets were validated.
Sequence numbers advance only when firmware successfully enqueues a packet.
Thus these counters check the stream after enqueueing; they do not claim that a
nonblocking producer can enqueue unlimited application samples without loss.
The saturation producer retries while the ring is full instead of advancing its
sequence number.

Raw values and test metadata are in [rtt-throughput-results.json](rtt-throughput-results.json).

## Setup

- Horco CMSIS-DAP, selector `ef1a:74e5:7844331832`.
- STM32H723VGTx. Reset HSI clock at 64 MHz; RTT data in uncached DTCM.
- probe-rs library 0.32.0. Installed CLI 0.32.0, commit `48f5e4d`.
- 16 KiB RTT up-channel buffer, `NoBlockSkip` mode.
- Continuous 256-byte packets: four-byte magic, u32 sequence, u32 millisecond
  timestamp, and a known payload pattern.
- The fixture drives no board peripherals. It spends its foreground loop
  producing packets; this is not a simultaneous vehicle-control workload.
- Direct reader drains RTT using a reusable 16 KiB host buffer and validates
  complete packets across read boundaries.
- The DAP test uses `probe-rs dap-server` over stdio, request `attach`, and
  `BinaryLE` RTT channel configuration. A Python DAP client performs the channel
  window handshake, decodes the events, and validates the original packets.
  Its measurement excludes the first baseline event after warmup.
- Normal VS Code webview rendering, other logs, multiple RTT channels, and
  firmware application load are not included in the DAP measurement.

The current fixture's RTT control block is at `0x20004008`. Rebuilding may move it;
extract `_SEGGER_RTT` from the ELF instead of assuming this address permanently.

## What this means for tuning

Eight f32 values plus a u32 timestamp and u32 sequence number occupy 40 bytes.
At 1 kHz they need 40,000 bytes/s before framing and other traffic. The 1 MHz
measurement does not meet that budget. At 4 MHz the measured DAP rate is about
2.2 times the budget; at 10 MHz it is about 4.8 times the budget.
This is bandwidth headroom, not a demonstrated 1 kHz application capture.
Smaller records, packet framing, firmware CPU cost, other logs, debugger activity,
and buffer management can change the result.

Start with 4 MHz for the first prototype. Keep UI redraws slower than sample
acquisition and retain firmware timestamps. Reserve bandwidth for debugger and
log traffic. Use the fast setting when the actual board/cabling remains reliable.

## DAP transport finding

Probe-rs 0.32's BinaryLE output is text: each byte becomes `0xNN`. The Python
receiver reconstructed the byte stream without corruption. It costs four text
characters per binary byte before JSON framing. At the fast measured rate that
is about 766 kB/s of event text. The separate stream did not substantially reduce
throughput in these runs, but VS Code delivery/rendering costs still need a test.

The measured route can carry telemetry without another probe owner or a new DAP
server. RTT command input is still a separate gap in the 0.32 DAP interface;
the library supports down channels, but its dispatcher exposes no down-channel
send request. See the [sharing investigation](probe-rs-sharing.md) and
[working options](live-watch-options.md).

## Limits of the result

We measured throughput, not a bound on interruption or firmware execution jitter.
The direct receiver observed `Running` before and after its captures. The RTT
reader itself does not issue halt/resume operations, but these checks do not
rule out transient debug effects or measure bus contention. DAP session setup
can also affect execution state.

Large saturated reads are slow individually. Direct-read p95 durations were
about 485 ms, 181 ms, and 88 ms at the three clocks. These are host RTT read-call
durations, not CPU halted times. Smaller host read batches may improve debugger
responsiveness at some throughput cost. RTT buffer capacity and delivery latency
must be tuned separately from maximum byte rate.

No live down-channel edits, firmware API integration, cycle-level timing,
long-duration soak test, or real VS Code receiver was tested here.

## Reproduce

Build the dedicated firmware from `firmware/log-lab`:

```sh
cargo build --release --locked --bin rtt-throughput
```

Flash the ELF to a test board using the standalone CLI, then build the receiver
from the repository root:

```sh
cargo build --locked -p tuning-studio-cli --example rtt-throughput

target/debug/examples/rtt-throughput \
  --probe ef1a:74e5:7844331832 --chip STM32H723VGTx \
  --speed 4000 --rtt-address 0x20004008 --seconds 10
```

The firmware source is
`firmware/log-lab/src/bin/rtt-throughput.rs`. The direct reader and its fragmented
packet-decoder test are in
`crates/tuning-studio-cli/examples/rtt-throughput.rs`.
The local DAP harness and detailed adapter output are retained under
`target/analysis/rtt-throughput/`; that directory is ignored by Git.
