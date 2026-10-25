# tuning-studio-dwarf

ELF and DWARF inspection for C, C++ and Rust embedded firmware. The
`tuning-studio` CLI uses it to resolve symbol paths, decode values and find
Embassy tasks. It parses a firmware image without opening a probe.

## Use it

```toml
[dependencies]
tuning-studio-dwarf = { git = "https://github.com/hxyulin/tuning-studio" }
```

The Rust library name is **`studio_dwarf`**.

```rust
use studio_dwarf::ElfParser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let elf = ElfParser::parse("firmware.elf")?;
    for symbol in elf.get_variables().into_iter().filter(|s| s.is_readable()) {
        println!("{:#x} {}", symbol.address, symbol.demangled_name);
    }
    Ok(())
}
```

## What it provides

- `ElfParser` and `ElfInfo`: symbols, memory sections, source locations and DWARF
  diagnostics, with C++ and Rust demangling.
- `tree`: typed nodes identified by `NodeRef` and `Step`, including structs,
  arrays, tagged enums, pointers, bitfields and bounded borrowed-string previews.
- `type_table`: shared type definitions for custom inspection tools.
- `rebuild`: compare tracked symbols after recompiling firmware.
- `tasks`, `task_stats` and `task_trace`: Embassy task layout discovery and
  decoders for the supported statistics and event-ring formats.

Keep debug information in the ELF (`debug = 2`, `strip = false` for Rust release
profiles). Parsing cannot restore optimized-away locals. Pointer resolution and
live previews need a memory reader from the caller; the parser does not connect
to hardware. Compiler-generated async layouts may change between toolchains.
Use diagnostics and unsupported-node results rather than assuming every type
can be interpreted. Numeric plot values use `f64`; exact inspection text is
left to the caller.

## Development

From the [repository](https://github.com/hxyulin/tuning-studio):

```sh
cargo test -p tuning-studio-dwarf
STUDIO_DWARF_ELF=path/to/firmware cargo test -p tuning-studio-dwarf -- --ignored
```

See the [fixture guide](https://github.com/hxyulin/tuning-studio/blob/main/crates/studio-dwarf/tests/fixtures/README.md)
for C, C++ and Rust examples, including the runnable inspector lab.
The parser originated in `datavis-rs`; provenance is recorded in the source.

Part of [Tuning Studio](https://github.com/hxyulin/tuning-studio). Host-only (`std`),
MIT licensed. The API changes between versions; use the same version as the
other Tuning Studio crates.
