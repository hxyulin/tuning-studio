//! DWARF Debug Info Parser using TypeTable
//!
//! This module parses DWARF debug information from ELF files and populates
//! a TypeTable with type definitions. It uses a simpler approach than the
//! original elf_parser.rs by leveraging the TypeTable's TypeId-based references.
//!
//! The parsing is done in a single pass with deferred resolution:
//! 1. Allocate TypeIds for all DIEs as we encounter them
//! 2. Parse type definitions, using TypeIds for references
//! 3. After all types are parsed, resolve forward declarations
//!
//! Template support is included by parsing DW_TAG_template_type_parameter
//! and DW_TAG_template_value_parameter DIEs.

mod attrs;
mod location;
mod status;
mod type_dies;
mod variables;

pub use status::{DwarfDiagnostics, VariableStatus};
use variables::{DeferredVariable, PendingSymbol, VariableInfo};

use super::error::Result;
use super::type_table::TypeTable;
use super::types::{DwarfTypeKey, GlobalTypeKey, TypeDef, TypeId};
use gimli::{
    AttributeValue, DebuggingInformationEntry, Dwarf, EndianSlice, ReaderOffset, RunTimeEndian,
    Unit,
};
use object::{Object, ObjectSection};
use std::borrow::Cow;

/// A parsed symbol with type information
#[derive(Debug, Clone)]
pub(crate) struct ParsedSymbol {
    pub name: String,
    pub mangled_name: Option<String>,
    pub address: u64,
    pub size: u64,
    pub type_id: TypeId,
    pub is_global: bool,
    /// Status indicating why a variable can or cannot be read
    pub status: VariableStatus,
}

/// Result of parsing DWARF info
#[derive(Debug)]
pub(crate) struct DwarfParseResult {
    pub type_table: TypeTable,
    pub symbols: Vec<ParsedSymbol>,
    /// Mapping from variable name (including mangled names) to TypeId
    /// This includes variables without addresses that couldn't become full symbols
    pub name_to_type: std::collections::HashMap<String, TypeId>,
    /// Diagnostic statistics about variable parsing
    pub diagnostics: DwarfDiagnostics,
}

/// DWARF parser that populates a TypeTable
pub(crate) struct DwarfParser<'a, R: gimli::Reader> {
    dwarf: &'a Dwarf<R>,
    type_table: TypeTable,
    /// Parsed variables pending final resolution
    pending_symbols: Vec<PendingSymbol>,
    /// Cache of variable info by global DIE offset for abstract_origin/specification resolution
    variable_cache: std::collections::HashMap<usize, VariableInfo>,
    /// Variables needing deferred resolution (spec/origin DIE appeared before target)
    deferred_variables: Vec<DeferredVariable>,
    /// Whether the compile unit being walked is Rust (`DW_LANG_Rust`)
    unit_is_rust: bool,
}

type Reader<'a> = EndianSlice<'a, RunTimeEndian>;

impl<'a> DwarfParser<'a, Reader<'a>> {
    /// Parse DWARF info from raw bytes
    pub fn parse_bytes(data: &'a [u8]) -> Result<DwarfParseResult> {
        let file = object::File::parse(data)?;

        let endian = if file.is_little_endian() {
            RunTimeEndian::Little
        } else {
            RunTimeEndian::Big
        };

        // Load DWARF sections
        let load_section = |id: gimli::SectionId| -> std::result::Result<Cow<[u8]>, gimli::Error> {
            Ok(file
                .section_by_name(id.name())
                .and_then(|s| s.data().ok())
                .map(Cow::Borrowed)
                .unwrap_or(Cow::Borrowed(&[])))
        };

        let dwarf_sections: gimli::DwarfSections<Cow<[u8]>> =
            gimli::DwarfSections::load(load_section)?;

        let dwarf = dwarf_sections.borrow(|section| EndianSlice::new(section, endian));

        let mut parser = DwarfParser {
            dwarf: &dwarf,
            type_table: TypeTable::new(),
            pending_symbols: Vec::new(),
            variable_cache: std::collections::HashMap::new(),
            deferred_variables: Vec::new(),
            unit_is_rust: false,
        };

        parser.parse_all_units()?;

        Ok(parser.finish())
    }

    /// Parse all compilation units
    fn parse_all_units(&mut self) -> Result<()> {
        let mut units = self.dwarf.units();
        let mut unit_index = 0usize;

        while let Some(header) = units.next()? {
            let unit = self.dwarf.unit(header)?;

            self.parse_unit(&unit, unit_index)?;
            unit_index += 1;
        }

        Ok(())
    }

    /// Parse a single compilation unit
    fn parse_unit(&mut self, unit: &Unit<Reader<'a>>, unit_index: usize) -> Result<()> {
        self.unit_is_rust = unit
            .entries_tree(None)
            .ok()
            .and_then(|mut tree| {
                let root = tree.root().ok()?;
                root.entry().attr_value(gimli::DW_AT_language)
            })
            .is_some_and(|v| matches!(v, AttributeValue::Language(gimli::DW_LANG_Rust)));

        let mut entries = unit.entries();

        while let Some(entry) = entries.next_dfs()? {
            self.parse_entry(unit, unit_index, entry)?;
        }

        Ok(())
    }

    /// Parse a single DIE
    fn parse_entry(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Result<()> {
        let offset: usize = entry.offset().0.into_u64() as usize;
        let key = DwarfTypeKey::new(unit_index, offset);

        // Also compute the global offset for this DIE so we can register it as an alias.
        // ARM compilers often use DebugInfoRef (global offsets) instead of UnitRef (local offsets).
        let global_offset = entry
            .offset()
            .to_debug_info_offset(&unit.header)
            .map(|o| o.0.into_u64() as usize);

        match entry.tag() {
            // Type DIEs
            gimli::DW_TAG_base_type => {
                self.parse_base_type(unit, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_pointer_type => {
                self.parse_pointer_type(unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_reference_type | gimli::DW_TAG_rvalue_reference_type => {
                self.parse_reference_type(unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_const_type => {
                self.parse_const_type(unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_volatile_type => {
                self.parse_volatile_type(unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_restrict_type => {
                self.parse_restrict_type(unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_typedef => {
                self.parse_typedef(unit, unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_array_type => {
                self.parse_array_type(unit, unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_structure_type | gimli::DW_TAG_class_type => {
                self.parse_struct_type(unit, unit_index, key, entry, false);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_union_type => {
                self.parse_struct_type(unit, unit_index, key, entry, true);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_enumeration_type => {
                self.parse_enum_type(unit, unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_subroutine_type => {
                self.parse_subroutine_type(unit_index, key, entry);
                self.register_global_alias(key, global_offset);
            }
            gimli::DW_TAG_unspecified_type => {
                // Usually represents void
                self.type_table.insert_for_key(key, TypeDef::Void);
                self.register_global_alias(key, global_offset);
            }

            // Variable/symbol DIEs
            gimli::DW_TAG_variable => {
                self.parse_variable(unit, unit_index, key, entry);
                return Ok(());
            }

            _ => {
                // Ignore other tags
                return Ok(());
            }
        }

        // Every arm that reaches here defined a type under `key`
        if self.unit_is_rust
            && let Some(id) = self.type_table.get_by_dwarf_key(key)
        {
            self.type_table.mark_rust(id);
            // rustc names base and pointer types (`u16`, `&mut T`, `fn() -> !`)
            if matches!(
                entry.tag(),
                gimli::DW_TAG_base_type | gimli::DW_TAG_pointer_type
            ) && let Some(name) = self.get_name(unit, entry)
            {
                self.type_table.set_source_name(id, name);
            }
        }

        // A reference by global offset may have reached this DIE first
        if let (Some(id), Some(offset)) = (self.type_table.get_by_dwarf_key(key), global_offset) {
            let global = GlobalTypeKey::global(offset).to_dwarf_key();
            if let Some(alias) = self.type_table.get_by_dwarf_key(global) {
                self.type_table.fill_placeholder(alias, id);
            }
        }

        Ok(())
    }

    /// Finish parsing and return the result
    fn finish(mut self) -> DwarfParseResult {
        // Resolve pending specifications (second pass)
        // These are specifications that referenced declarations that appeared later in DWARF
        self.resolve_pending_specifications();

        // Resolve forward declarations
        self.type_table.resolve_forward_declarations();

        // Resolve inherited members
        self.resolve_inherited_members();

        // Build name-to-type mapping for ALL variables (including those without addresses)
        // and separate out symbols with addresses
        let mut name_to_type = std::collections::HashMap::new();
        let mut symbols = Vec::new();
        let mut diagnostics = DwarfDiagnostics::default();

        for pending in self.pending_symbols {
            let type_id = pending
                .type_key
                .and_then(|k| self.type_table.get_by_dwarf_key(k))
                .unwrap_or(TypeId::INVALID);

            // Track diagnostics based on status
            diagnostics.record(&pending.status);

            if !type_id.is_valid() {
                diagnostics.unresolved_types += 1;
                continue;
            }

            // Add to name-to-type mapping (for all variables)
            name_to_type.insert(pending.name.clone(), type_id);
            if let Some(ref mangled) = pending.mangled_name {
                name_to_type.insert(mangled.clone(), type_id);
            }

            // Get address from status (if readable)
            let address = pending.status.address().unwrap_or(0);

            // Get size from type if not specified
            let size = if pending.size > 0 {
                pending.size
            } else {
                self.type_table.type_size(type_id).unwrap_or(0)
            };

            symbols.push(ParsedSymbol {
                name: pending.name,
                mangled_name: pending.mangled_name,
                address,
                size,
                type_id,
                is_global: pending.is_global,
                status: pending.status,
            });
        }

        let stats = self.type_table.stats();

        tracing::debug!(
            "DWARF parsing complete: {} types, {} structs, {} forward decls ({} unresolved)",
            stats.total_types,
            stats.structs,
            stats.forward_decls,
            stats.unresolved_forward_decls,
        );

        // Log detailed diagnostics
        tracing::info!(
            "DWARF variables: {} total | {} readable | {} optimized | {} register-only | {} artificial",
            diagnostics.total_variables,
            diagnostics.with_valid_address + diagnostics.address_zero,
            diagnostics.optimized_out,
            diagnostics.register_only,
            diagnostics.artificial,
        );

        if diagnostics.local_variables > 0
            || diagnostics.extern_declarations > 0
            || diagnostics.compile_time_constants > 0
        {
            tracing::debug!(
                "DWARF details: {} local | {} extern | {} const",
                diagnostics.local_variables,
                diagnostics.extern_declarations,
                diagnostics.compile_time_constants,
            );
        }

        if diagnostics.no_location > 0
            || diagnostics.unresolved_types > 0
            || diagnostics.implicit_value > 0
            || diagnostics.implicit_pointer > 0
            || diagnostics.multi_piece > 0
        {
            tracing::debug!(
                "DWARF edge cases: {} no location | {} unresolved types | {} implicit | {} implicit ptr | {} multi-piece | {} addr zero",
                diagnostics.no_location,
                diagnostics.unresolved_types,
                diagnostics.implicit_value,
                diagnostics.implicit_pointer,
                diagnostics.multi_piece,
                diagnostics.address_zero,
            );
        }

        // Log info about variable cache and deferred resolution
        tracing::debug!(
            "DWARF variable cache: {} entries, {} deferred resolutions",
            self.variable_cache.len(),
            self.deferred_variables.len()
        );

        DwarfParseResult {
            type_table: self.type_table,
            symbols,
            name_to_type,
            diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test ELF fixtures are embedded at compile time
    const TEST_ARM_ELF: &[u8] = include_bytes!("../tests/fixtures/test_arm.elf");
    const TEST_STRUCT_ELF: &[u8] = include_bytes!("../tests/fixtures/test_struct.elf");
    const TEST_POINTER_ELF: &[u8] = include_bytes!("../tests/fixtures/test_pointer.elf");
    const TEST_COMPLEX_C_ELF: &[u8] = include_bytes!("../tests/fixtures/test_complex_c.elf");
    const TEST_CPP_ELF: &[u8] = include_bytes!("../tests/fixtures/test_cpp.elf");

    #[test]
    fn test_dwarf_type_key() {
        let key1 = DwarfTypeKey::new(0, 100);
        let key2 = DwarfTypeKey::new(0, 100);
        let key3 = DwarfTypeKey::new(1, 100);

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_parse_simple_variables() {
        let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Failed to parse ELF");

        // Check we found some variables
        assert!(!result.symbols.is_empty(), "Should find variables");

        // Look for specific variables we know are in the test ELF
        let global_counter = result.symbols.iter().find(|s| s.name == "global_counter");
        assert!(
            global_counter.is_some(),
            "Should find global_counter variable"
        );

        if let Some(var) = global_counter {
            assert!(
                var.status.is_readable(),
                "global_counter should be readable"
            );
            assert!(var.address > 0, "global_counter should have valid address");
        }

        let sensor_data = result.symbols.iter().find(|s| s.name == "sensor_data");
        assert!(sensor_data.is_some(), "Should find sensor_data variable");
    }

    #[test]
    fn test_parse_enum_types() {
        let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Failed to parse ELF");

        // Look for the enum variable
        let current_state = result.symbols.iter().find(|s| s.name == "current_state");
        assert!(
            current_state.is_some(),
            "Should find current_state enum variable"
        );

        // Check that we parsed enum types
        let stats = result.type_table.stats();
        assert!(stats.enums > 0, "Should have parsed enum types");
    }

    #[test]
    fn test_parse_struct_types() {
        let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Failed to parse ELF");

        // Look for struct variables
        let sensor_struct = result.symbols.iter().find(|s| s.name == "sensor_struct");
        assert!(
            sensor_struct.is_some(),
            "Should find sensor_struct variable"
        );

        // Check that we parsed struct types
        let stats = result.type_table.stats();
        assert!(stats.structs > 0, "Should have parsed struct types");

        // Check that structs have members
        if let Some(var) = sensor_struct {
            let members = result.type_table.members(var.type_id);
            assert!(members.is_some(), "Struct should have members");
            if let Some(members) = members {
                assert!(
                    members.len() >= 3,
                    "SensorData should have at least 3 fields (x, y, value)"
                );
            }
        }
    }

    #[test]
    fn test_parse_nested_structs() {
        let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Failed to parse ELF");

        // Look for device_config which contains a nested SensorData
        let device_config = result.symbols.iter().find(|s| s.name == "device_config");

        if let Some(var) = device_config {
            let members = result.type_table.members(var.type_id);
            assert!(members.is_some(), "DeviceConfig should have members");
            if let Some(members) = members {
                // Should have id, sensor (nested struct), enabled
                assert!(
                    members.len() >= 3,
                    "DeviceConfig should have at least 3 fields"
                );
            }
        }
    }

    #[test]
    fn test_parse_array_types() {
        let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Failed to parse ELF");

        // Look for the buffer array
        let buffer = result.symbols.iter().find(|s| s.name == "buffer");
        assert!(buffer.is_some(), "Should find buffer array");

        // Check array type parsing
        let stats = result.type_table.stats();
        assert!(stats.arrays > 0, "Should have parsed array types");
    }

    #[test]
    fn test_parse_pointer_types() {
        let result = DwarfParser::parse_bytes(TEST_POINTER_ELF).expect("Failed to parse ELF");

        // Look for pointer variables
        let data_ptr = result.symbols.iter().find(|s| s.name == "data_ptr");
        assert!(data_ptr.is_some(), "Should find data_ptr variable");

        let float_ptr = result.symbols.iter().find(|s| s.name == "float_ptr");
        assert!(float_ptr.is_some(), "Should find float_ptr variable");

        let double_ptr = result.symbols.iter().find(|s| s.name == "double_ptr");
        assert!(double_ptr.is_some(), "Should find double_ptr variable");

        // Check pointer type parsing
        let stats = result.type_table.stats();
        assert!(stats.pointers > 0, "Should have parsed pointer types");
    }

    #[test]
    fn test_parse_null_pointer() {
        let result = DwarfParser::parse_bytes(TEST_POINTER_ELF).expect("Failed to parse ELF");

        // Look for null_ptr
        let null_ptr = result.symbols.iter().find(|s| s.name == "null_ptr");
        assert!(null_ptr.is_some(), "Should find null_ptr variable");
    }

    #[test]
    fn test_parse_complex_c_types() {
        let result = DwarfParser::parse_bytes(TEST_COMPLEX_C_ELF).expect("Failed to parse ELF");

        // Test packed struct
        let packed_data = result.symbols.iter().find(|s| s.name == "packed_data");
        assert!(packed_data.is_some(), "Should find packed_data");

        // Test bitfield struct
        let bitfield_data = result.symbols.iter().find(|s| s.name == "bitfield_data");
        assert!(bitfield_data.is_some(), "Should find bitfield_data");

        // Test union
        let data_union = result.symbols.iter().find(|s| s.name == "data_union");
        assert!(data_union.is_some(), "Should find data_union");

        // Check union type parsing
        let stats = result.type_table.stats();
        assert!(stats.unions > 0, "Should have parsed union types");
    }

    #[test]
    fn test_parse_nested_structs_complex() {
        let result = DwarfParser::parse_bytes(TEST_COMPLEX_C_ELF).expect("Failed to parse ELF");

        // Test deeply nested struct
        let nested = result.symbols.iter().find(|s| s.name == "nested");
        assert!(nested.is_some(), "Should find nested struct variable");
    }

    #[test]
    fn test_parse_cpp_namespaces() {
        let result = DwarfParser::parse_bytes(TEST_CPP_ELF).expect("Failed to parse ELF");

        // Look for variables in namespaces
        // C++ name mangling means we might not find them by simple name
        // but they should be in the symbols list
        assert!(!result.symbols.is_empty(), "Should find C++ variables");

        // Check that temp_sensor exists (may be mangled)
        let temp_sensor = result
            .symbols
            .iter()
            .find(|s| s.name.contains("temp_sensor") || s.name == "temp_sensor");
        assert!(temp_sensor.is_some(), "Should find temp_sensor variable");
    }

    #[test]
    fn test_parse_cpp_classes() {
        let result = DwarfParser::parse_bytes(TEST_CPP_ELF).expect("Failed to parse ELF");

        // Check that we parsed class/struct types
        let stats = result.type_table.stats();
        assert!(stats.structs > 0, "Should have parsed C++ class types");
    }

    #[test]
    fn test_parse_cpp_templates() {
        let result = DwarfParser::parse_bytes(TEST_CPP_ELF).expect("Failed to parse ELF");

        // Look for template instantiations (int_container, float_container)
        let int_container = result
            .symbols
            .iter()
            .find(|s| s.name.contains("int_container"));
        assert!(
            int_container.is_some(),
            "Should find int_container template instantiation"
        );

        let float_container = result
            .symbols
            .iter()
            .find(|s| s.name.contains("float_container"));
        assert!(
            float_container.is_some(),
            "Should find float_container template instantiation"
        );
    }

    #[test]
    fn test_variable_status_classification() {
        let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Failed to parse ELF");

        // Count variables by status
        let readable_count = result
            .symbols
            .iter()
            .filter(|s| s.status.is_readable())
            .count();

        assert!(
            readable_count > 0,
            "Should have at least some readable variables"
        );

        // Check diagnostics
        assert!(
            result.diagnostics.total_variables > 0,
            "Should have found variables"
        );
        assert!(
            result.diagnostics.with_valid_address > 0,
            "Should have variables with valid addresses"
        );
    }

    #[test]
    fn test_type_table_completeness() {
        let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Failed to parse ELF");

        let stats = result.type_table.stats();

        // We should have parsed various type categories
        assert!(stats.primitives > 0, "Should have primitive types");
        assert!(stats.total_types > 0, "Should have total types");

        // Type table should not be empty
        assert!(
            !result.type_table.is_empty(),
            "Type table should not be empty"
        );
    }

    #[test]
    fn test_variable_addresses_are_valid() {
        let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Failed to parse ELF");

        for symbol in &result.symbols {
            if symbol.status.is_readable()
                && let Some(addr) = symbol.status.address()
            {
                // On ARM Cortex-M, RAM typically starts at 0x20000000
                // Flash at 0x08000000
                assert!(
                    addr >= 0x08000000 || addr >= 0x20000000 || addr == 0,
                    "Address should be in valid memory range: {:x}",
                    addr
                );
            }
        }
    }

    #[test]
    fn test_name_to_type_mapping() {
        let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Failed to parse ELF");

        // name_to_type should have entries
        assert!(
            !result.name_to_type.is_empty(),
            "Should have name-to-type mappings"
        );

        // Each symbol should have a type mapping
        for symbol in &result.symbols {
            assert!(
                result.name_to_type.contains_key(&symbol.name)
                    || symbol
                        .mangled_name
                        .as_ref()
                        .is_some_and(|n| result.name_to_type.contains_key(n)),
                "Symbol {} should have type mapping",
                symbol.name
            );
        }
    }

    #[test]
    fn test_diagnostics_accuracy() {
        let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Failed to parse ELF");

        let diag = &result.diagnostics;

        // Total should equal sum of categories (approximately)
        let categorized = diag.with_valid_address
            + diag.optimized_out
            + diag.local_variables
            + diag.extern_declarations
            + diag.compile_time_constants
            + diag.no_location
            + diag.unresolved_types
            + diag.address_zero
            + diag.register_only
            + diag.implicit_value
            + diag.implicit_pointer
            + diag.multi_piece
            + diag.artificial;

        // Should be close (may have some uncategorized)
        assert!(
            categorized <= diag.total_variables,
            "Categorized count should not exceed total"
        );
        assert!(
            diag.total_variables > 0,
            "Should have processed some variables"
        );
    }

    #[test]
    fn test_parse_all_fixtures_without_panic() {
        // Ensure all test fixtures can be parsed without panicking
        let fixtures = [
            ("test_arm.elf", TEST_ARM_ELF),
            ("test_struct.elf", TEST_STRUCT_ELF),
            ("test_pointer.elf", TEST_POINTER_ELF),
            ("test_complex_c.elf", TEST_COMPLEX_C_ELF),
            ("test_cpp.elf", TEST_CPP_ELF),
        ];

        for (name, data) in &fixtures {
            let result = DwarfParser::parse_bytes(data);
            assert!(
                result.is_ok(),
                "Failed to parse {}: {:?}",
                name,
                result.err()
            );
        }
    }
}
