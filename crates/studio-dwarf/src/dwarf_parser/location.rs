//! Evaluation of `DW_AT_location` expressions into a [`VariableStatus`].

use super::status::VariableStatus;
use super::{DwarfParser, Reader};
use gimli::{AttributeValue, DebuggingInformationEntry, Unit};

/// Step limit for location expressions, so a looping `DW_OP_skip` cannot hang the parser.
const MAX_EXPR_ITERATIONS: u32 = 10_000;

impl<'a> DwarfParser<'a, Reader<'a>> {
    /// Get detailed variable status including why a variable can or cannot be read.
    ///
    /// This provides diagnostic information about:
    /// - Artificial (compiler-generated) variables
    /// - Compile-time constants (have value but no runtime storage)
    /// - External declarations (defined in another compilation unit)
    /// - Optimized out variables (no location in DWARF)
    /// - Local variables (require runtime context like registers or stack)
    /// - Valid static addresses (including address 0 on embedded targets)
    pub(super) fn get_variable_status(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> VariableStatus {
        // 0. Check for artificial (compiler-generated) FIRST
        // These include: `this` pointers, VLA bounds, closure captures, temporaries
        if self.has_artificial_attr(entry) {
            return VariableStatus::Artificial;
        }

        // 1. Check for compile-time constant (has value but may not have location)
        let has_const_value = entry.attr_value(gimli::DW_AT_const_value).is_some();
        let has_location = entry.attr_value(gimli::DW_AT_location).is_some();

        if has_const_value && !has_location {
            let value = self.get_const_value(entry);
            return VariableStatus::CompileTimeConstant { value };
        }

        // 2. Check for extern declaration (external + no location)
        let is_external = self.has_external_attr(entry);
        if is_external && !has_location {
            return VariableStatus::ExternDeclaration;
        }

        // 3. No location attribute = likely optimized out
        if !has_location {
            return VariableStatus::OptimizedOut;
        }

        // 4. Evaluate the location expression to get the address
        self.evaluate_location_to_status(unit, entry)
    }

    /// Evaluate a location attribute and return detailed status
    pub(super) fn evaluate_location_to_status(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> VariableStatus {
        let attr = match entry.attr_value(gimli::DW_AT_location) {
            Some(attr) => attr,
            None => return VariableStatus::NoLocation,
        };

        match attr {
            // Expression-based location (most common for global variables)
            AttributeValue::Exprloc(expr) => self.evaluate_expr_to_status(unit, &expr),

            // Direct address value (some older DWARF or simple cases)
            AttributeValue::Addr(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid { address: addr }
                }
            }

            // Indexed address (DWARF 5)
            AttributeValue::DebugAddrIndex(index) => match self.dwarf.address(unit, index) {
                Ok(0) => VariableStatus::AddressZero,
                Ok(addr) => VariableStatus::Valid { address: addr },
                Err(_) => VariableStatus::NoLocation,
            },

            // Location list reference
            AttributeValue::LocationListsRef(offset) => {
                match self.evaluate_location_list(unit, offset) {
                    Some(0) => VariableStatus::AddressZero,
                    Some(addr) => VariableStatus::Valid { address: addr },
                    None => VariableStatus::LocalVariable {
                        reason: "Location list with no static entry".to_string(),
                    },
                }
            }

            // Offset into location lists (DWARF 5)
            AttributeValue::DebugLocListsIndex(index) => {
                if let Ok(offset) = self.dwarf.locations_offset(unit, index) {
                    match self.evaluate_location_list(unit, offset) {
                        Some(0) => VariableStatus::AddressZero,
                        Some(addr) => VariableStatus::Valid { address: addr },
                        None => VariableStatus::LocalVariable {
                            reason: "Location list with no static entry".to_string(),
                        },
                    }
                } else {
                    VariableStatus::NoLocation
                }
            }

            // Block containing location expression (older DWARF)
            AttributeValue::Block(block) => {
                let expr = gimli::Expression(block);
                self.evaluate_expr_to_status(unit, &expr)
            }

            // Data forms that might contain addresses directly
            AttributeValue::Udata(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid { address: addr }
                }
            }
            AttributeValue::Data1(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid {
                        address: addr as u64,
                    }
                }
            }
            AttributeValue::Data2(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid {
                        address: addr as u64,
                    }
                }
            }
            AttributeValue::Data4(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid {
                        address: addr as u64,
                    }
                }
            }
            AttributeValue::Data8(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid { address: addr }
                }
            }
            AttributeValue::Sdata(addr) => {
                if addr == 0 {
                    VariableStatus::AddressZero
                } else {
                    VariableStatus::Valid {
                        address: addr as u64,
                    }
                }
            }

            _ => VariableStatus::NoLocation,
        }
    }

    /// Evaluate a DWARF expression and return detailed status
    pub(super) fn evaluate_expr_to_status(
        &self,
        unit: &Unit<Reader<'a>>,
        expr: &gimli::Expression<Reader<'a>>,
    ) -> VariableStatus {
        let mut evaluation = (*expr).evaluation(unit.encoding());
        evaluation.set_max_iterations(MAX_EXPR_ITERATIONS);

        let mut result = match evaluation.evaluate() {
            Ok(r) => r,
            Err(_) => return VariableStatus::NoLocation,
        };

        loop {
            match result {
                // Evaluation complete - extract the address from the result
                gimli::EvaluationResult::Complete => {
                    let pieces = evaluation.result();

                    // Empty result = optimized out
                    if pieces.is_empty() {
                        return VariableStatus::OptimizedOut;
                    }

                    // Multi-piece: variable split across locations (DW_OP_piece/DW_OP_bit_piece)
                    if pieces.len() > 1 {
                        let has_address = pieces
                            .iter()
                            .any(|p| matches!(p.location, gimli::Location::Address { .. }));
                        return VariableStatus::MultiPiece { has_address };
                    }

                    // Single piece: check the location type
                    return match pieces[0].location {
                        gimli::Location::Address { address } => {
                            if address == 0 {
                                VariableStatus::AddressZero
                            } else {
                                VariableStatus::Valid { address }
                            }
                        }
                        gimli::Location::Register { register } => VariableStatus::RegisterOnly {
                            register: register.0,
                        },
                        gimli::Location::Value { .. } => VariableStatus::ImplicitValue,
                        gimli::Location::ImplicitPointer { .. } => VariableStatus::ImplicitPointer,
                        gimli::Location::Empty => VariableStatus::OptimizedOut,
                        // Bytes is used for raw data in some DWARF versions
                        gimli::Location::Bytes { .. } => VariableStatus::ImplicitValue,
                    };
                }

                // Needs an address from .debug_addr section
                gimli::EvaluationResult::RequiresIndexedAddress { index, relocate: _ } => {
                    match self.dwarf.address(unit, index) {
                        Ok(address) => match evaluation.resume_with_indexed_address(address) {
                            Ok(r) => result = r,
                            Err(_) => return VariableStatus::NoLocation,
                        },
                        Err(_) => return VariableStatus::NoLocation,
                    }
                }

                // Needs address relocation
                gimli::EvaluationResult::RequiresRelocatedAddress(address) => {
                    match evaluation.resume_with_relocated_address(address) {
                        Ok(r) => result = r,
                        Err(_) => return VariableStatus::NoLocation,
                    }
                }

                // Needs base type information
                gimli::EvaluationResult::RequiresBaseType(_) => {
                    let value_type = gimli::ValueType::Generic;
                    match evaluation.resume_with_base_type(value_type) {
                        Ok(r) => result = r,
                        Err(_) => return VariableStatus::NoLocation,
                    }
                }

                // Runtime requirements - these indicate local variables
                gimli::EvaluationResult::RequiresRegister { register, .. } => {
                    return VariableStatus::LocalVariable {
                        reason: format!("Requires register {}", register.0),
                    };
                }
                gimli::EvaluationResult::RequiresFrameBase => {
                    return VariableStatus::LocalVariable {
                        reason: "Stack variable (requires frame base)".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresMemory { .. } => {
                    return VariableStatus::LocalVariable {
                        reason: "Requires memory read at runtime".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresTls(_) => {
                    return VariableStatus::LocalVariable {
                        reason: "Thread-local storage variable".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresCallFrameCfa => {
                    return VariableStatus::LocalVariable {
                        reason: "Requires call frame CFA".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresAtLocation(_) => {
                    return VariableStatus::LocalVariable {
                        reason: "Requires DW_AT_location from another DIE".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresEntryValue(_) => {
                    return VariableStatus::LocalVariable {
                        reason: "Requires entry value (function parameter)".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresParameterRef(_) => {
                    return VariableStatus::LocalVariable {
                        reason: "Requires parameter reference".to_string(),
                    };
                }
                gimli::EvaluationResult::RequiresWasmLocal { .. }
                | gimli::EvaluationResult::RequiresWasmGlobal { .. }
                | gimli::EvaluationResult::RequiresWasmStack { .. } => {
                    return VariableStatus::LocalVariable {
                        reason: "Requires a WebAssembly runtime value".to_string(),
                    };
                }
            }
        }
    }

    /// Evaluate a location list to find the address for a global/static variable.
    /// For global variables, we look for an entry that covers "any" address or
    /// the first valid location expression.
    pub(super) fn evaluate_location_list(
        &self,
        unit: &Unit<Reader<'a>>,
        offset: gimli::LocationListsOffset<<Reader<'a> as gimli::Reader>::Offset>,
    ) -> Option<u64> {
        let mut locations = self.dwarf.locations(unit, offset).ok()?;

        // Iterate through location list entries
        while let Ok(Some(entry)) = locations.next() {
            // Try to evaluate this location expression
            if let Some(addr) = self.evaluate_simple_location_expr(unit, &entry.data) {
                return Some(addr);
            }
        }
        None
    }

    /// Evaluate a DWARF location expression to extract a static address.
    ///
    /// Uses gimli's Evaluation API for proper DWARF expression handling.
    /// This correctly handles all DWARF operations including:
    /// - DW_OP_addr: Direct address (GCC, Clang)
    /// - DW_OP_addrx: Indexed address in .debug_addr (DWARF 5, GCC 11+, Clang 14+)
    /// - DW_OP_GNU_addr_index: GNU extension for indexed addresses
    /// - DW_OP_constN + DW_OP_plus_uconst: Computed addresses (some ARM compilers)
    /// - All stack-based arithmetic and logical operations
    ///
    /// Note: We only handle static/global addresses. When the evaluator requests
    /// runtime information (registers, memory, frame base, etc.), we return None
    /// as these are for local variables that can't be resolved statically.
    pub(super) fn evaluate_simple_location_expr(
        &self,
        unit: &Unit<Reader<'a>>,
        expr: &gimli::Expression<Reader<'a>>,
    ) -> Option<u64> {
        let mut evaluation = (*expr).evaluation(unit.encoding());
        evaluation.set_max_iterations(MAX_EXPR_ITERATIONS);

        // Run the evaluation loop
        let mut result = evaluation.evaluate().ok()?;

        loop {
            match result {
                // Evaluation complete - extract the address from the result
                gimli::EvaluationResult::Complete => {
                    let pieces = evaluation.result();
                    // We expect a single piece with an address for global variables
                    if pieces.len() == 1 {
                        // Value on stack without Location means the address is the value itself
                        if let gimli::Location::Address { address } = pieces[0].location {
                            return Some(address);
                        }
                    }
                    // For multi-piece or non-address locations, try to get address from first piece
                    for piece in pieces {
                        if let gimli::Location::Address { address } = piece.location {
                            return Some(address);
                        }
                    }
                    return None;
                }

                // Needs an address from .debug_addr section (DW_OP_addrx, DW_OP_GNU_addr_index)
                gimli::EvaluationResult::RequiresIndexedAddress { index, relocate: _ } => {
                    let address = self.dwarf.address(unit, index).ok()?;
                    result = evaluation.resume_with_indexed_address(address).ok()?;
                }

                // Needs address relocation (for position-independent code)
                gimli::EvaluationResult::RequiresRelocatedAddress(address) => {
                    // For our purposes, we don't need to relocate - the address is already correct
                    result = evaluation.resume_with_relocated_address(address).ok()?;
                }

                // Needs base type information for typed operations
                gimli::EvaluationResult::RequiresBaseType(offset) => {
                    // For address computation, we can use a generic 64-bit unsigned type
                    let value_type = gimli::ValueType::Generic;
                    result = evaluation.resume_with_base_type(value_type).ok()?;
                    let _ = offset; // Silence unused warning
                }

                // Runtime requirements - these indicate local variables, not static addresses
                // We can't evaluate these without runtime context
                gimli::EvaluationResult::RequiresMemory { .. }
                | gimli::EvaluationResult::RequiresRegister { .. }
                | gimli::EvaluationResult::RequiresFrameBase
                | gimli::EvaluationResult::RequiresTls(_)
                | gimli::EvaluationResult::RequiresCallFrameCfa
                | gimli::EvaluationResult::RequiresAtLocation(_)
                | gimli::EvaluationResult::RequiresEntryValue(_)
                | gimli::EvaluationResult::RequiresParameterRef(_)
                | gimli::EvaluationResult::RequiresWasmLocal { .. }
                | gimli::EvaluationResult::RequiresWasmGlobal { .. }
                | gimli::EvaluationResult::RequiresWasmStack { .. } => {
                    // These are for local variables - we can't resolve them statically
                    return None;
                }
            }
        }
    }
}
