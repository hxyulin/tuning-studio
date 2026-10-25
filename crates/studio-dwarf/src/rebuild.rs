//! Re-resolve tracked symbols against a rebuilt ELF.
//!
//! Model: `datavis-rs` `detect_variable_changes` (`src/frontend/mod.rs:1030-1099`),
//! lifted out of the UI and keyed by full symbol path rather than display name.

use crate::elf::ElfInfo;
use crate::variable_type::VariableType;

/// A symbol the host is watching, as it resolved in the previous ELF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedSymbol {
    /// Full demangled path (e.g. `firmware::gimbal::PITCH_STATE`).
    pub path: String,
    pub address: u64,
    pub var_type: VariableType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolChange {
    AddressChanged {
        path: String,
        old_address: u64,
        new_address: u64,
    },
    TypeChanged {
        path: String,
        old_type: VariableType,
        new_type: VariableType,
        new_type_name: String,
    },
    NotFound {
        path: String,
    },
}

/// Compare tracked symbols with `elf`. Unchanged symbols produce nothing; a
/// symbol whose address and type both moved produces two entries.
pub fn diff_symbols(tracked: &[TrackedSymbol], elf: &ElfInfo) -> Vec<SymbolChange> {
    let mut changes = Vec::new();
    for t in tracked {
        let Some(sym) = elf.find_symbol(&t.path) else {
            changes.push(SymbolChange::NotFound {
                path: t.path.clone(),
            });
            continue;
        };
        if sym.address != t.address {
            changes.push(SymbolChange::AddressChanged {
                path: t.path.clone(),
                old_address: t.address,
                new_address: sym.address,
            });
        }
        let new_type = elf.infer_variable_type_for_symbol(sym);
        if new_type != t.var_type {
            changes.push(SymbolChange::TypeChanged {
                path: t.path.clone(),
                old_type: t.var_type,
                new_type,
                new_type_name: elf.get_symbol_type_name(sym),
            });
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::ElfParser;

    const TEST_ARM_ELF: &[u8] = include_bytes!("../tests/fixtures/test_arm.elf");

    fn tracked(elf: &ElfInfo, name: &str) -> TrackedSymbol {
        let sym = elf.find_symbol(name).expect("fixture symbol");
        TrackedSymbol {
            path: sym.demangled_name.clone(),
            address: sym.address,
            var_type: elf.infer_variable_type_for_symbol(sym),
        }
    }

    #[test]
    fn unchanged_symbol_reports_nothing() {
        let elf = ElfParser::parse_bytes(TEST_ARM_ELF, "test_arm.elf").unwrap();
        let t = tracked(&elf, "global_counter");
        assert!(diff_symbols(&[t], &elf).is_empty());
    }

    #[test]
    fn moved_retyped_and_missing_symbols_are_reported() {
        let elf = ElfParser::parse_bytes(TEST_ARM_ELF, "test_arm.elf").unwrap();
        let mut moved = tracked(&elf, "global_counter");
        let real_address = moved.address;
        moved.address += 4;
        moved.var_type = VariableType::U8;
        let gone = TrackedSymbol {
            path: "no_such_symbol".into(),
            address: 0x2000_0000,
            var_type: VariableType::U32,
        };

        let changes = diff_symbols(&[moved, gone], &elf);
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert_eq!(
            changes[0],
            SymbolChange::AddressChanged {
                path: "global_counter".into(),
                old_address: real_address + 4,
                new_address: real_address,
            }
        );
        assert!(matches!(
            changes[1],
            SymbolChange::TypeChanged {
                old_type: VariableType::U8,
                ..
            }
        ));
        assert_eq!(
            changes[2],
            SymbolChange::NotFound {
                path: "no_such_symbol".into()
            }
        );
    }
}
