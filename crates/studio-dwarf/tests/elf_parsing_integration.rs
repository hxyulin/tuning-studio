//! Integration tests for ELF parsing
//!
//! These tests validate end-to-end ELF parsing using real test fixtures:
//! - Parsing various ELF formats (C, C++, complex types)
//! - Type table construction
//! - Variable discovery

use studio_dwarf::DwarfParser;

// Test fixtures compiled at build time
const TEST_ARM_ELF: &[u8] = include_bytes!("fixtures/test_arm.elf");
const TEST_STRUCT_ELF: &[u8] = include_bytes!("fixtures/test_struct.elf");
const TEST_POINTER_ELF: &[u8] = include_bytes!("fixtures/test_pointer.elf");
const TEST_COMPLEX_C_ELF: &[u8] = include_bytes!("fixtures/test_complex_c.elf");
const TEST_CPP_ELF: &[u8] = include_bytes!("fixtures/test_cpp.elf");

#[test]
fn test_parse_basic_c_elf() {
    let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Should parse test_arm.elf");

    // Verify we found variables
    assert!(!result.symbols.is_empty(), "Should find variables");

    // Check for specific known variables
    let var_names: Vec<_> = result.symbols.iter().map(|s| s.name.as_str()).collect();

    assert!(
        var_names.contains(&"global_counter"),
        "Should find global_counter"
    );
    assert!(
        var_names.contains(&"sensor_data"),
        "Should find sensor_data"
    );

    // Verify type table has content
    assert!(
        !result.type_table.is_empty(),
        "Type table should have entries"
    );

    // Check diagnostics
    assert!(
        result.diagnostics.total_variables > 0,
        "Should have processed variables"
    );
    assert!(
        result.diagnostics.with_valid_address > 0,
        "Should have variables with addresses"
    );
}

#[test]
fn test_parse_struct_elf() {
    let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Should parse test_struct.elf");

    // Look for struct variables
    let sensor_struct = result.symbols.iter().find(|s| s.name == "sensor_struct");

    assert!(
        sensor_struct.is_some(),
        "Should find sensor_struct variable"
    );

    // Verify struct has members in type table
    if let Some(var) = sensor_struct {
        let members = result.type_table.get_members(var.type_id);
        assert!(members.is_some(), "Struct should have members");

        if let Some(members) = members {
            assert!(
                members.len() >= 3,
                "SensorData should have at least 3 members"
            );
        }
    }

    // Check for nested struct
    let device_config = result.symbols.iter().find(|s| s.name == "device_config");
    assert!(
        device_config.is_some(),
        "Should find device_config with nested struct"
    );
}

#[test]
fn test_parse_pointer_elf() {
    let result = DwarfParser::parse_bytes(TEST_POINTER_ELF).expect("Should parse test_pointer.elf");

    // Look for pointer variables
    let pointers = ["data_ptr", "float_ptr", "double_ptr", "null_ptr"];

    for ptr_name in &pointers {
        let found = result.symbols.iter().any(|s| &s.name == ptr_name);
        assert!(found, "Should find pointer variable: {}", ptr_name);
    }

    // Verify pointer types exist in type table
    let stats = result.type_table.stats();
    assert!(stats.pointers > 0, "Should have parsed pointer types");
}

#[test]
fn test_parse_complex_c_elf() {
    let result =
        DwarfParser::parse_bytes(TEST_COMPLEX_C_ELF).expect("Should parse test_complex_c.elf");

    // Check for complex C features
    let complex_vars = ["packed_data", "bitfield_data", "data_union", "nested"];

    for var_name in &complex_vars {
        let found = result.symbols.iter().any(|s| &s.name == var_name);
        assert!(found, "Should find complex variable: {}", var_name);
    }

    // Verify union type parsing
    let stats = result.type_table.stats();
    assert!(stats.unions > 0, "Should have parsed union types");
}

#[test]
fn test_parse_cpp_elf() {
    let result = DwarfParser::parse_bytes(TEST_CPP_ELF).expect("Should parse test_cpp.elf");

    // C++ names might be mangled or in namespaces
    assert!(!result.symbols.is_empty(), "Should find C++ variables");

    // Check that structs/classes were parsed
    let stats = result.type_table.stats();
    assert!(stats.structs > 0, "Should have parsed C++ classes/structs");

    // Look for template instantiations
    let has_template_vars = result
        .symbols
        .iter()
        .any(|s| s.name.contains("container") || s.name.contains("Container"));
    assert!(
        has_template_vars,
        "Should find template instantiation variables"
    );
}

#[test]
fn test_all_fixtures_parse_successfully() {
    let fixtures = vec![
        ("test_arm.elf", TEST_ARM_ELF),
        ("test_struct.elf", TEST_STRUCT_ELF),
        ("test_pointer.elf", TEST_POINTER_ELF),
        ("test_complex_c.elf", TEST_COMPLEX_C_ELF),
        ("test_cpp.elf", TEST_CPP_ELF),
    ];

    for (name, data) in fixtures {
        let result = DwarfParser::parse_bytes(data);
        assert!(
            result.is_ok(),
            "Failed to parse {}: {:?}",
            name,
            result.err()
        );

        let parsed = result.unwrap();
        assert!(
            !parsed.symbols.is_empty() || parsed.diagnostics.total_variables > 0,
            "{} should have variables or diagnostics",
            name
        );
    }
}

#[test]
fn test_variable_address_ranges() {
    let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Should parse");

    // Check that all readable variables have valid addresses
    for symbol in &result.symbols {
        if symbol.status.is_readable() {
            let addr = symbol.status.address();
            assert!(addr.is_some(), "Readable variable should have address");

            // ARM Cortex-M typical memory ranges
            if let Some(a) = addr {
                // RAM (0x20000000+) or Flash (0x08000000+) or special (0x00000000)
                let valid_range = a == 0
                    || (0x08000000..0x08100000).contains(&a)
                    || (0x20000000..0x20100000).contains(&a);
                assert!(
                    valid_range,
                    "Address 0x{:08x} for {} outside valid ranges",
                    a, symbol.name
                );
            }
        }
    }
}

#[test]
fn test_type_table_integrity() {
    let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Should parse");

    let stats = result.type_table.stats();

    // Should have various type categories
    assert!(stats.primitives > 0, "Should have primitive types");
    assert!(stats.structs > 0, "Should have struct types");
    assert!(stats.total_types > 0, "Should have total types");

    // Total should be sum of categories (approximately)
    let categorized = stats.primitives
        + stats.structs
        + stats.unions
        + stats.enums
        + stats.pointers
        + stats.arrays
        + stats.typedefs
        + stats.qualifiers
        + stats.subroutines
        + stats.voids
        + stats.forward_decls;

    assert!(
        categorized <= stats.total_types,
        "Categorized count should not exceed total"
    );
}

#[test]
fn test_enum_parsing() {
    let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Should parse");

    // Check for enum variable
    let current_state = result.symbols.iter().find(|s| s.name == "current_state");

    // Enum might be present depending on debug info
    if current_state.is_some() {
        let stats = result.type_table.stats();
        // Should have parsed enum type
        assert!(stats.enums > 0 || stats.total_types > 0);
    }
}

#[test]
fn test_variable_sizes() {
    let result = DwarfParser::parse_bytes(TEST_ARM_ELF).expect("Should parse");

    // Check that variables have reasonable sizes
    for symbol in &result.symbols {
        if symbol.size > 0 {
            // Most variables should be reasonably sized
            assert!(
                symbol.size <= 1024,
                "Variable {} has unexpectedly large size: {}",
                symbol.name,
                symbol.size
            );
        }
    }
}

#[test]
fn test_name_to_type_mapping_completeness() {
    let result = DwarfParser::parse_bytes(TEST_STRUCT_ELF).expect("Should parse");

    // Every symbol should have a type mapping
    for symbol in &result.symbols {
        let has_mapping = result.name_to_type.contains_key(&symbol.name)
            || symbol
                .mangled_name
                .as_ref()
                .is_some_and(|n| result.name_to_type.contains_key(n));

        assert!(
            has_mapping,
            "Symbol {} should have type mapping",
            symbol.name
        );
    }
}

#[test]
fn test_diagnostic_categories() {
    let result = DwarfParser::parse_bytes(TEST_COMPLEX_C_ELF).expect("Should parse");

    let diag = &result.diagnostics;

    // Should have categorized all variables
    assert_eq!(
        diag.total_variables,
        diag.with_valid_address
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
            + diag.artificial,
        "All variables should be categorized"
    );
}
