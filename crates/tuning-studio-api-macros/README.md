# tuning-studio-api-macros

`#[derive(Watchable)]` for [`tuning-studio-api`](../tuning-studio-api). Depend
on `tuning-studio-api` instead, which re-exports the derive with its default
`derive` feature.

```rust
use tuning_studio_api::{Watchable, watch};

#[derive(Watchable)]
struct Motor {
    speed: f32,
    pi: Pi,              // derives Watchable too
    #[watch(skip)]
    driver: CanDriver,   // nothing to show
}

watch!("motor": Motor = *motor);
```

The derive generates `MotorSnapshot`, a `Copy` struct with the kept fields under
the same names, each holding that field's snapshot. `watch!` stores the
snapshot. It accepts named, tuple and unit structs without generic parameters.
A `Copy` type is already `Watchable`, so deriving it there conflicts.
