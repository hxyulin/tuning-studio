//! GDB remote server, copied from probe-rs-tools 0.32.0
//! (`src/bin/probe-rs/cmd/gdb_server`), which is MIT OR Apache-2.0 licensed and
//! publishes no library. Its `RuntimeTarget` shares the session through a
//! `FairMutex` and polls without blocking, so the session loop drives it between
//! its own commands. Changes: `RuntimeTarget::is_connected`, and the snapshot test
//! and the `probe-rs gdb` command are left out.

mod arch;
mod target;

pub(crate) use target::RuntimeTarget as Server;
