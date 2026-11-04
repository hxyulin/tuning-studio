# Live Watch timing at 2 Hz on the Horco test board

Measured 2026-11-04 on the same Horco CMSIS-DAP and STM32H723VGTx used for the
RTT throughput trial. Firmware was replaced with the timing fixture under the
user's authorization to overwrite this test board.

## Findings

The existing probe-rs 0.32 DAP sleeping-core memory-read path did not meet a
sub-millisecond interruption budget in these tests. Reading at 2 Hz reduced how
often the target was disturbed, but did not bound the length of a disturbance.

The table shows the largest observed **extra interrupt-service gap**, after
subtracting the normal 1,000 us interval. These are firmware-side observations,
not host request latencies or exact electrical measurements of halt duration.

| SWD speed | DAP, 4 bytes | DAP, 64 bytes | DAP, 256 bytes |
| --- | ---: | ---: | ---: |
| 1 MHz | 21.044 ms | 32.113 ms | 42.791 ms |
| 4 MHz | 14.407 ms | 19.951 ms | 31.367 ms |
| 10 MHz | 21.237 ms | 94.921 ms | 18.100 ms |

Each cell covers 20 read requests scheduled 500 ms apart, with the target sleeping
in WFI between 1 ms interrupts. The 94.921 ms observation occurred in one 64-byte
capture at 10 MHz. These are short runs and maximum observations, not statistical
latency guarantees. A higher SWD clock did not eliminate host/probe scheduling
variation. The result does not establish the behavior of other probes or hosts.

For a busy-loop control, 20 DAP reads of 64 bytes at 2 Hz produced no increase over
the 1,000 us maximum IRQ gap at any of the three speeds. This is consistent with
the adapter's explicit sleep-related halt workaround rather than memory access
inherently requiring a halt.

For a second control, Tuning Studio's existing AP background reader, within one
persistent session, read 64 bytes at 2 Hz while firmware slept. All three clocks
again retained a 1,000 us maximum IRQ gap. No extra service delay was detected at
the fixture's measurement resolution. This is evidence that this board can
support a low-rate Live Watch without the DAP workaround or an RTT producer.
It is not proof of zero bus contention or zero execution overhead.

## Measurement method and checks

The fixture in `firmware/log-lab/src/bin/live-watch-timing.rs` uses the reset
64 MHz HSI clock. SysTick generates a nominal interrupt each millisecond.
TIM2 counts at 1 MHz, with its debug-freeze bit cleared. Each interrupt records
the TIM2 elapsed time since the previous interrupt, and retains the largest gap.
The target's foreground loop can either sleep in WFI or spin for the control test.
The watched block resides in DTCM and exposes an epoch request/acknowledgement,
the maximum gap, counters, and padding for different read sizes.

Before each capture, a host epoch request asks firmware to clear its maximum
at the next interrupt. That reset occurs after servicing the request so setup
writes and preceding session attachment do not contaminate the new capture.
After the final test read, a diagnostic read retrieves the retained maximum;
that diagnostic read's own interruption occurs after the statistic it returns.

The script separately validated the timer by pausing through the debugger,
reading TIM2, waiting at least 100 ms on the host, and reading TIM2 again before
resuming. TIM2 advanced about 118, 218, and 115 ms in the three runs, agreeing
with host elapsed times to about 1%. The entire manual halt, including queued
debugger reads, produced much larger recorded IRQ gaps of about 436, 492, and
381 ms. This confirms the timer continues during halt and the fixture detects
execution interruption. It does not make the IRQ-gap method a precise
measurement of the physical halt/resume edges.

No-poll baselines after reset showed maximum IRQ gaps of 1,000 us. The busy-loop
and persistent-session background-read controls also retained that baseline.
The reported excess gaps include interrupt-service effects and should be treated
as observed application disturbance, not precise per-request halted time.

## Host request latency is different

For 64-byte DAP reads while sleeping, median round-trip times were approximately
46.9 ms at 1 MHz, 37.5 ms at 4 MHz, and 41.9 ms at 10 MHz. The largest observed
request at 10 MHz was 116.3 ms. Queueing and adapter polling contribute to those
numbers outside the actual interrupted interval. They must not be reported as
CPU halt durations.

The DAP measurements use the installed probe-rs 0.32.0 CLI over stdio and a Python
client, not a running VS Code extension. The current implementation's sleep
workaround is visible in the [request handler](https://github.com/probe-rs/probe-rs/blob/v0.32.0/probe-rs-tools/src/bin/probe-rs/cmd/dap_server/server/debugger.rs).

The raw results, including each request's host latency, calibration values, and
observed cumulative maxima, are in
[live-watch-timing-results.json](live-watch-timing-results.json).
Local Python DAP harnesses and CLI capture logs remain under the ignored
`target/analysis/live-watch-timing/` directory.

## Attachment control

An initial comparison opened a new CLI attachment for the watch capture after
arming the fixture, then another for the diagnostic read. It recorded larger
maxima, but the maximum was already present at the first watch sample and stayed
unchanged during polling. Attachment/teardown can disturb execution. Those
exploratory numbers are excluded from the final background-reader comparison.

The final comparison starts one `tuning-studio attach` session per clock,
arms the fixture only after that session is ready, and routes set/watch/read
commands through it. Each session is stopped using its owner token after capture.
Twenty 64-byte watch samples take approximately 9.5 seconds between the first and
last sample. This matches 2 Hz scheduling; it is not a ten-second throughput test.

## Implications

Low-rate Live Watch need not require firmware telemetry code. Raw background
memory reads with DWARF decoding can serve it on this board. Live snapshots remain
best-effort for changing data, and task-local lifetime/state limitations remain.

For an integration where probe-rs owns the debugger session, investigate an
upstream non-halting memory-read operation or target-specific handling of the
sleep workaround. The target has demonstrated the needed background access;
sharing the owner and its request path remains the integration work. Do not
remove the workaround globally without checking targets that require it.

RTT plots still need firmware to collect and publish their samples. This can be
an optional capture path, independent of raw Live Watch. Neither RTT nor its
protocol alone supplies arbitrary task locals. RTT input also needs a firmware
command handler if it will carry tuning edits.

All test sessions were closed. The test board is left with the timing fixture
loaded, sleeping between interrupts. The RTT throughput fixture from the earlier
trial is no longer the flashed image.
