# Select lab

STM32H723 firmware that stresses reads of async state. One Embassy task awaits
`select(sine(), cosine())`. Each future counts its iterations, looks up the
sine or cosine of its count's phase in a table, and yields, so the executor
switches between them every iteration. An iteration of both takes about 305
cycles, 4.8 µs at the reset 64 MHz HSI clock with the instruction cache on.

Each future keeps a `Sample { count, value }` across its `.await`, so the
sample sits in the task's future, and also publishes it with `watch!`.
`value` is always `sin` or `cos` of `count % 1024 * 2π / 1024`, so every read of a
sample can be checked.

Two `Param`s change the load. `SPIN` adds cycles to each iteration, and
`WATCH_EVERY` writes the `watch!` values only every that many iterations. The
futures read both once per 1024 iterations.

## Run

```sh
cd firmware/select-lab
cargo build --release
tuning-studio flash --chip STM32H723VGTx target/thumbv7em-none-eabihf/release/select-lab
```

From the repository root, read the future and both `watch!` values as fast as
the probe can, and check every sample:

```sh
E=firmware/select-lab/target/thumbv7em-none-eabihf/release/select-lab
tuning-studio watch --chip STM32H723VGTx --elf $E --speed 10000 --interval 0 --count 1000 \
    "select_lab::waves::POOL[task 0].future" waves.sine waves.cosine \
    | python3 firmware/select-lab/check.py
tuning-studio set --chip STM32H723VGTx --elf $E select_lab::WATCH_EVERY 64
```

## Results

Measured on 2026-10-26 with the Horco CMSIS-DAP probe at 10 MHz SWD unless noted.
A sample is wrong when its value is not the function of its count.

**Task locals are not readable while the task runs.** Reading the future at
full speed, 1999 of 2000 samples were wrong. A read takes longer than several
iterations, so `count` and `value` come from different iterations; the value
was typically two counts ahead. Slower firmware helps, but no setting made
this path reliable:

| Firmware | Iteration | Wrong samples |
|---|---|---|
| Table, I-cache on | 4.8 µs | 99.9% |
| `sinf`, I-cache off, 4 MHz SWD | 126 µs | 18–19% |
| `sinf`, `SPIN` 10000 | 439 µs | 5–11% |
| `sinf`, `SPIN` 100000, 4 MHz SWD | 3.2 ms | 0.85% |

At slow iterations the wrong samples were reads that landed mid-poll, after
`count` was updated and before `value` was.

**`watch!` values were never wrong**, in over 20,000 samples across all runs.
A read is retried when it overlapped a write, and after 10 failed attempts it
shows `<changed during each of 10 reads>`. Whether a read gets through depends
on how often the firmware writes, compared with how long the probe takes to
read the value. For this 16-byte value that window was about 25 µs at 10 MHz
SWD, 115 µs at 4 MHz, and 225 µs at 1 MHz. At full speed:

| `WATCH_EVERY` | Write every | First attempt succeeds | Unreadable samples |
|---|---|---|---|
| 1 | 4.8 µs | 0% | 100% |
| 4 | 19 µs | 0% | 100% |
| 16 | 76 µs | 62% | 0% |
| 64 | 305 µs | 90% | 0% |
| 256 | 1.2 ms | 97% | 0% |
| 1024 | 4.9 ms | 100% | 0% |

**Rate.** Each location costs at least one USB round trip, about 1.1 ms:

| What | Reads per second |
|---|---|
| One `watch!` value, 10 MHz SWD | 850–880 |
| The 48-byte future, 10 MHz SWD | 840–920 |
| The future, 1 MHz SWD | 450 |

Reading three locations per tick divides the rate by three. A sample that
needs 10 attempts costs 10 round trips: with all `watch!` reads failing, three
locations ran at 47 reads per second.

The 10 MHz results need `--speed 10000`, since the default is 4000 kHz. This
probe was no faster at 24 MHz.

**The data cache does not change this.** A build with the data in AXI SRAM and
the D-cache on (CCR bit 16 set, MPU off, so the region was write-back cached)
read the same as DTCM: every `watch!` sample was consistent and current, and
every `Param` request reached the firmware without cache maintenance. The
CLI's AP reaches memory through the Cortex-M7's debug port, which goes through
the cache.
