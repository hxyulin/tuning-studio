//! Whether a variable has a readable address, and counts of each outcome.

/// Status of a variable's address resolution
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum VariableStatus {
    /// Variable has a valid static address
    Valid { address: u64 },
    /// Variable was optimized out (has type but no location)
    OptimizedOut,
    /// Variable is a local requiring runtime context
    LocalVariable { reason: String },
    /// Variable is an extern declaration (defined elsewhere)
    ExternDeclaration,
    /// Variable is a compile-time constant
    CompileTimeConstant { value: Option<i64> },
    /// Variable's type could not be resolved
    UnresolvedType { type_name: Option<String> },
    /// No location information in DWARF
    NoLocation,
    /// Location evaluated to address 0 (may be valid on embedded)
    AddressZero,
    /// Value is in a register only (no memory address)
    RegisterOnly { register: u16 },
    /// Value is implicit/computed (DW_OP_stack_value, DW_OP_implicit_value)
    ImplicitValue,
    /// Pointer to optimized-out data (DW_OP_implicit_pointer)
    ImplicitPointer,
    /// Variable split across multiple locations (DW_OP_piece)
    MultiPiece { has_address: bool },
    /// Compiler-generated variable (DW_AT_artificial)
    Artificial,
}

impl VariableStatus {
    /// Check if this variable is readable (has a valid address)
    pub fn is_readable(&self) -> bool {
        matches!(
            self,
            VariableStatus::Valid { .. }
                | VariableStatus::AddressZero
                | VariableStatus::MultiPiece { has_address: true }
        )
    }

    /// Get the address if available
    pub fn address(&self) -> Option<u64> {
        match self {
            VariableStatus::Valid { address } => Some(*address),
            VariableStatus::AddressZero => Some(0),
            VariableStatus::MultiPiece { has_address: true } => {
                // For multi-piece with address, we return address 0 as placeholder
                // The actual address handling happens during evaluation
                Some(0)
            }
            _ => None,
        }
    }

    /// Get a human-readable reason for the status
    pub fn reason(&self) -> &'static str {
        match self {
            VariableStatus::Valid { .. } => "Valid address",
            VariableStatus::OptimizedOut => "Optimized out by compiler",
            VariableStatus::LocalVariable { .. } => "Local variable (requires runtime context)",
            VariableStatus::ExternDeclaration => "External declaration (defined in another unit)",
            VariableStatus::CompileTimeConstant { .. } => {
                "Compile-time constant (no runtime storage)"
            }
            VariableStatus::UnresolvedType { .. } => "Type information unavailable",
            VariableStatus::NoLocation => "No location information in debug info",
            VariableStatus::AddressZero => "Address is 0 (may be valid on embedded targets)",
            VariableStatus::RegisterOnly { .. } => "Value in register only (no memory address)",
            VariableStatus::ImplicitValue => "Implicit value (computed, no storage)",
            VariableStatus::ImplicitPointer => "Implicit pointer (points to optimized-out data)",
            VariableStatus::MultiPiece { has_address: true } => {
                "Split across locations (partial address)"
            }
            VariableStatus::MultiPiece { has_address: false } => {
                "Split across registers/implicit values"
            }
            VariableStatus::Artificial => "Compiler-generated variable",
        }
    }

    /// Get detailed reason including any additional context
    pub fn detailed_reason(&self) -> String {
        match self {
            VariableStatus::LocalVariable { reason } => {
                format!("Local variable: {}", reason)
            }
            VariableStatus::CompileTimeConstant { value: Some(v) } => {
                format!("Compile-time constant: value = {}", v)
            }
            VariableStatus::UnresolvedType {
                type_name: Some(name),
            } => {
                format!("Unresolved type: {}", name)
            }
            _ => self.reason().to_string(),
        }
    }
}

/// Diagnostic statistics from DWARF parsing
#[derive(Debug, Default, Clone)]
pub struct DwarfDiagnostics {
    /// Total number of variables found in DWARF
    pub total_variables: usize,
    /// Variables with valid static addresses
    pub with_valid_address: usize,
    /// Variables optimized out by the compiler
    pub optimized_out: usize,
    /// Local variables requiring runtime context
    pub local_variables: usize,
    /// External declarations (defined elsewhere)
    pub extern_declarations: usize,
    /// Compile-time constants with no runtime storage
    pub compile_time_constants: usize,
    /// Variables with no location information
    pub no_location: usize,
    /// Variables with unresolved types
    pub unresolved_types: usize,
    /// Variables with address 0 (may be valid on embedded)
    pub address_zero: usize,
    /// Variables in registers only (no memory address)
    pub register_only: usize,
    /// Variables with implicit/computed values
    pub implicit_value: usize,
    /// Variables with implicit pointers
    pub implicit_pointer: usize,
    /// Variables split across multiple locations
    pub multi_piece: usize,
    /// Compiler-generated (artificial) variables
    pub artificial: usize,
}

impl DwarfDiagnostics {
    /// Increment the appropriate counter based on variable status
    pub fn record(&mut self, status: &VariableStatus) {
        self.total_variables += 1;
        match status {
            VariableStatus::Valid { .. } => self.with_valid_address += 1,
            VariableStatus::OptimizedOut => self.optimized_out += 1,
            VariableStatus::LocalVariable { .. } => self.local_variables += 1,
            VariableStatus::ExternDeclaration => self.extern_declarations += 1,
            VariableStatus::CompileTimeConstant { .. } => self.compile_time_constants += 1,
            VariableStatus::UnresolvedType { .. } => self.unresolved_types += 1,
            VariableStatus::NoLocation => self.no_location += 1,
            VariableStatus::AddressZero => self.address_zero += 1,
            VariableStatus::RegisterOnly { .. } => self.register_only += 1,
            VariableStatus::ImplicitValue => self.implicit_value += 1,
            VariableStatus::ImplicitPointer => self.implicit_pointer += 1,
            VariableStatus::MultiPiece { .. } => self.multi_piece += 1,
            VariableStatus::Artificial => self.artificial += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_variable_status_methods() {
        // Test VariableStatus utility methods
        let valid_status = VariableStatus::Valid {
            address: 0x20000000,
        };
        assert!(valid_status.is_readable());
        assert_eq!(valid_status.address(), Some(0x20000000));
        assert_eq!(valid_status.reason(), "Valid address");

        let optimized_status = VariableStatus::OptimizedOut;
        assert!(!optimized_status.is_readable());
        assert_eq!(optimized_status.address(), None);
        assert_eq!(optimized_status.reason(), "Optimized out by compiler");

        let zero_status = VariableStatus::AddressZero;
        assert!(zero_status.is_readable());
        assert_eq!(zero_status.address(), Some(0));
    }
}
