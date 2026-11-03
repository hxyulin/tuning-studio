# Parameter fixture

`params_v2.elf` uses the current API layout without `target` or `max_step`.
It includes a persistent `tune_lab::KP` with key `motor.kp`.
The older `tune_lab.elf` remains for backward compatibility tests.

Regenerate from the repository root:

```sh
(cd firmware/tune-lab && cargo build --release --locked)
cp firmware/tune-lab/target/thumbv7em-none-eabihf/release/tune-lab crates/tuning-studio-cli/tests/fixtures/params_v2.elf
```

This image is parsed in host tests and does not need to be flashed.
