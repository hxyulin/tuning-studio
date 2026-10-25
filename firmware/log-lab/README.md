# Log lab

STM32H723 firmware that logs over RTT, for checking `tuning-studio run` and
`attach`. Up channel 0, named `defmt`, carries defmt frames with a millisecond
timestamp. Up channel 1 carries plain text. Every 500 ms it logs one defmt line,
cycling through debug, info, warn and error, and one text line.

Setting `PANIC` to a nonzero value makes it panic on the next tick. panic-probe
logs the message and then triggers a HardFault, which `run` catches and reports
as a halt.

It runs from the reset 64 MHz HSI clock with data in DTCM, which the D-cache
does not cover. Its Cargo runner is `tuning-studio run`, so with `tuning-studio`
on the `PATH` this flashes it, resets it and prints its logs. Flashing replaces
the board's firmware.

```sh
cd firmware/log-lab
cargo run --release
```

While it runs, other commands go to it from another terminal, with no `--chip`
or `--elf`: `tuning-studio read log_lab::MILLIS`, `tuning-studio set PANIC 1`
or `tuning-studio reset`. Ctrl-C detaches.
