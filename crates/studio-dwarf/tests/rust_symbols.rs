//! Rust firmware symbols: demangling, stable paths, Rust type names, enum layouts.
//!
//! Fixtures come from `tests/fixtures/rust_embedded`, built with the rm-embedded-rs
//! release profile for thumbv7em-none-eabihf, once with the default v0 mangling and
//! once with legacy mangling. See `tests/fixtures/README.md`.

use studio_dwarf::type_table::{MemberDef, TypeDef, VariantPart};
use studio_dwarf::{ElfInfo, ElfParser, SymbolType, TrackedSymbol, TypeId, diff_symbols};

const RUST_V0_ELF: &[u8] = include_bytes!("fixtures/rust_v0.elf");
const RUST_LEGACY_ELF: &[u8] = include_bytes!("fixtures/rust_legacy.elf");

const STATICS: &[(&str, &str)] = &[
    ("rust_fixture::telemetry::TX_FRAMES", "Atomic<u32>"),
    ("rust_fixture::telemetry::LINK_UP", "Atomic<bool>"),
    ("rust_fixture::telemetry::SAMPLES", "[u16; 8]"),
    ("rust_fixture::control::nested::TICKS", "u64"),
    (
        "rust_fixture::control::GIMBAL",
        "Shared<rust_fixture::control::Gimbal<4>>",
    ),
    ("C_COUNTER", "u32"),
];

fn both() -> [(&'static str, ElfInfo); 2] {
    [
        (
            "v0",
            ElfParser::parse_bytes(RUST_V0_ELF, "rust_v0.elf").unwrap(),
        ),
        (
            "legacy",
            ElfParser::parse_bytes(RUST_LEGACY_ELF, "rust_legacy.elf").unwrap(),
        ),
    ]
}

fn ram_variables(elf: &ElfInfo) -> Vec<&studio_dwarf::SymbolInfo> {
    let mut vars: Vec<_> = elf
        .get_variables()
        .into_iter()
        .filter(|s| (0x2000_0000..0x2001_0000).contains(&s.address))
        .collect();
    vars.sort_by_key(|s| s.address);
    vars
}

#[test]
fn statics_resolve_by_full_path_with_rust_type_names() {
    for (label, elf) in both() {
        for (path, type_name) in STATICS {
            let sym = elf
                .find_symbol(path)
                .unwrap_or_else(|| panic!("{label}: {path} not found"));
            assert_eq!(sym.symbol_type, SymbolType::Variable, "{label}: {path}");
            assert!(sym.is_readable(), "{label}: {path}");
            assert!(sym.writable, "{label}: {path} should be in RAM");
            assert_eq!(elf.get_symbol_type_name(sym), *type_name, "{label}: {path}");
        }
    }
}

#[test]
fn paths_carry_no_hashes() {
    for (label, elf) in both() {
        for sym in &elf.symbols {
            let p = &sym.demangled_name;
            assert!(!p.contains('['), "{label}: crate disambiguator in {p}");
            assert!(!p.contains(".llvm."), "{label}: LTO suffix in {p}");
            let last = p.rsplit("::").next().unwrap();
            let legacy_hash = last.len() == 17
                && last.starts_with('h')
                && last[1..].chars().all(|c| c.is_ascii_hexdigit());
            assert!(!legacy_hash, "{label}: legacy hash in {p}");
            assert!(
                !p.starts_with("anon.") && !p.starts_with(".L"),
                "{label}: {p}"
            );
        }
    }
}

#[test]
fn mangling_scheme_does_not_change_identity() {
    let [(_, v0), (_, legacy)] = both();
    let describe = |elf: &ElfInfo| -> Vec<(String, u64, String)> {
        ram_variables(elf)
            .into_iter()
            .map(|s| {
                (
                    s.demangled_name.clone(),
                    s.address,
                    elf.get_symbol_type_name(s),
                )
            })
            .collect()
    };
    assert_eq!(describe(&v0), describe(&legacy));

    // A watch list saved against one build re-resolves cleanly against the other.
    let tracked: Vec<_> = ram_variables(&v0)
        .into_iter()
        .map(|s| TrackedSymbol {
            path: s.demangled_name.clone(),
            address: s.address,
            var_type: v0.infer_variable_type_for_symbol(s),
        })
        .collect();
    assert_eq!(tracked.len(), STATICS.len());
    assert_eq!(diff_symbols(&tracked, &legacy), vec![]);
}

/// Walks `name` through structs, tuple wrappers (`__0`) and `UnsafeCell::value`.
fn member(elf: &ElfInfo, ty: TypeId, name: &str) -> MemberDef {
    let table = elf.type_table();
    match table.get(table.get_underlying(ty)) {
        Some(TypeDef::Struct(s)) => s
            .members
            .iter()
            .find(|m| m.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no member {name} in {}", table.type_name(ty))),
        other => panic!("{} is not a struct: {other:?}", table.type_name(ty)),
    }
}

fn variant_part(elf: &ElfInfo, ty: TypeId) -> VariantPart {
    let table = elf.type_table();
    match table.get(table.get_underlying(ty)) {
        Some(TypeDef::Struct(s)) => s.variant_part.clone().expect("variant part"),
        other => panic!("not an enum: {other:?}"),
    }
}

fn gimbal(elf: &ElfInfo) -> (u64, TypeId) {
    let sym = elf.find_symbol("rust_fixture::control::GIMBAL").unwrap();
    let cell = member(elf, sym.type_id.unwrap(), "__0");
    let value = member(elf, cell.type_id, "value");
    (sym.address + cell.offset + value.offset, value.type_id)
}

#[test]
fn nested_struct_members_have_offsets_and_types() {
    for (label, elf) in both() {
        let table = elf.type_table();
        let (_, g) = gimbal(&elf);
        assert_eq!(table.type_name(g), "Gimbal<4>", "{label}");
        assert_eq!(table.type_size(g), Some(80), "{label}");

        let pitch = member(&elf, g, "pitch");
        let kp = member(&elf, pitch.type_id, "kp");
        assert_eq!(table.type_name(kp.type_id), "f32", "{label}");

        let history = member(&elf, g, "history");
        assert_eq!(table.type_name(history.type_id), "[f32; 4]", "{label}");

        let offset = member(&elf, g, "offset");
        assert_eq!(table.type_name(offset.type_id), "(i16, i16)", "{label}");
        assert_eq!(member(&elf, offset.type_id, "__1").offset, 2, "{label}");

        let mode = member(&elf, g, "mode");
        match table.get(table.get_underlying(mode.type_id)) {
            Some(TypeDef::Enum(e)) => {
                let v: Vec<_> = e
                    .variants
                    .iter()
                    .map(|v| (v.name.as_str(), v.value))
                    .collect();
                assert_eq!(v, [("Idle", 0), ("Running", 1), ("Fault", 7)], "{label}");
            }
            other => panic!("{label}: Mode is {other:?}"),
        }
    }
}

#[test]
fn tagged_enum_exposes_discriminant_and_variants() {
    for (label, elf) in both() {
        let table = elf.type_table();
        let (_, g) = gimbal(&elf);
        let command = member(&elf, g, "command");
        assert!(table.is_expandable(command.type_id), "{label}");
        let part = variant_part(&elf, command.type_id);

        let discr = part.discriminant.as_ref().expect("tag member");
        assert_eq!(table.type_size(discr.type_id), Some(4), "{label}");

        let names: Vec<_> = part
            .variants
            .iter()
            .map(|v| (v.member.name.as_str(), v.discr_value))
            .collect();
        assert_eq!(
            names,
            [
                ("Stop", Some(0)),
                ("Velocity", Some(1)),
                ("Position", Some(2))
            ],
            "{label}"
        );

        let position = part.select(2).unwrap();
        assert_eq!(
            table.type_name(position.member.type_id),
            "Position",
            "{label}"
        );
        let target = member(&elf, position.member.type_id, "target");
        assert_eq!(table.type_name(target.type_id), "f32", "{label}");
        assert!(target.offset > discr.offset, "{label}: payload after tag");
    }
}

#[test]
fn niche_enum_falls_back_to_dataful_variant() {
    for (label, elf) in both() {
        let table = elf.type_table();
        let (_, g) = gimbal(&elf);
        let fault = member(&elf, g, "last_fault");
        assert_eq!(
            table.type_name(fault.type_id),
            "Option<core::num::nonzero::NonZero<u32>>",
            "{label}"
        );
        assert_eq!(
            table.type_size(fault.type_id),
            Some(4),
            "{label}: no separate tag"
        );

        let part = variant_part(&elf, fault.type_id);
        assert_eq!(part.select(0).unwrap().member.name, "None", "{label}");
        assert_eq!(part.select(42).unwrap().member.name, "Some", "{label}");
    }
}

/// Smoke test against a real firmware build:
/// `STUDIO_DWARF_ELF=path/to/firmware cargo test -p studio-dwarf -- --ignored`
#[test]
#[ignore = "needs STUDIO_DWARF_ELF"]
fn real_firmware_elf() {
    let path = std::env::var("STUDIO_DWARF_ELF").expect("STUDIO_DWARF_ELF");
    let elf = ElfParser::parse(&path).unwrap();
    let statics: Vec<_> = elf
        .get_variables()
        .into_iter()
        .filter(|s| s.is_readable() && s.type_id.is_some() && s.demangled_name.contains("::"))
        .collect();
    assert!(!statics.is_empty(), "no typed Rust statics in {path}");
    assert!(
        elf.symbols
            .iter()
            .filter(|s| s.section == ".defmt")
            .all(|s| !s.writable),
        "defmt metadata is not RAM"
    );
    for s in &statics {
        assert!(!s.demangled_name.contains('['), "{}", s.demangled_name);
        let _ = elf.get_symbol_type_name(s);
    }
    eprintln!("{} typed Rust statics", statics.len());
}
