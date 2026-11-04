# Native Live Watch validation

Tested on macOS on 2026-11-04 with the STM32H723VGTx test board and Horco
CMSIS-DAP probe `ef1a:74e5:7844331832`. The board ran the repository's
`firmware/tune-lab` release ELF. Firmware was flashed and verified before testing.
The native app attached at 4 MHz and polled every 500 ms.

## Checks on the board

- Connected from the native UI and read all ten declared API values.
- Edited `KI` from `0.5` to `0.0` and read it back through the CLI.
- Edited the `watch_mut!` value `motor.integral` to `0.25` and saw the change
  in the native app and in the simulated motor response.
- Opened a read-only `watch!` value's reason and copyable `watch_mut!` example.
- Expanded `motor.state` and its nested `pi` fields.
- Copied `KP` and pasted it into the Add field. Only the raw number `0.02`
  was copied.
- Queried real ELF autocomplete for `MOTOR` and added its symbol.
- Added an invalid watch while declared values kept updating, then removed it.
- Submitted an invalid number for `KP`, retained the input and failure reason,
  corrected it to `0.03`, and read back the accepted value.
- Locked and unlocked `KP`. Locked values showed a reason instead of an editor.
- Halted and resumed the target from the native toolbar.
- Closed and relaunched the app. Settings, added watch, and expansion survived.
  Closing stopped the owned CLI session.
- Replaced an added watch through its context menu with `MOTOR.pi.integral`.
- After fixing contiguous DWARF piece decoding, verified that raw
  `MOTOR.pi.integral`, `motor.integral`, and the expanded snapshot all read `0.25`.

The last two checks found a backend bug: a static described by adjacent address
pieces could resolve to address zero. The decoder now accepts adjacent whole-byte
memory pieces, preserves the canonical variable name alongside ELF shards, and
rejects unsupported split locations. A regression test uses the real fixture ELF.

## Automated checks

The following commands passed in this session:

```sh
cargo fmt --all
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo build -p tuning-studio-cli -p tuning-studio-desktop --locked
cargo doc --workspace --no-deps --locked
```

The workspace tests passed with one opt-in real-firmware ELF test ignored.
The new native app has three tests covering model behavior and socket replies.
UI interactions above were checked manually in the native app, not by those tests.

## Limits of this validation

Linux, reusing an externally owned session, probe removal during a write, and
multiple connected probes were not tested on hardware. Pointer and wrapper
decoding use the existing backend; this board session exercised nested structs
and scalar fields rather than every wrapper type. The app does not add plots,
RTT streaming, breakpoints, or a DAP server.
