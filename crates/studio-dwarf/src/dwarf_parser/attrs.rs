//! Attribute readers shared by the type and variable handlers.

use super::{DwarfParser, Reader};
use crate::types::*;
use gimli::{AttributeValue, DebuggingInformationEntry, ReaderOffset, Unit, UnitOffset};

impl<'a> DwarfParser<'a, Reader<'a>> {
    // ==================== Attribute Helpers ====================

    pub(super) fn get_name(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<String> {
        let attr = entry.attr_value(gimli::DW_AT_name)?;
        self.attr_to_string(unit, &attr)
    }

    /// `DW_AT_decl_file` and `DW_AT_decl_line`, the file named through the
    /// unit's line program.
    pub(super) fn decl_location(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<SourceLocation> {
        let line = entry.attr_value(gimli::DW_AT_decl_line)?.udata_value()?;
        let index = match entry.attr_value(gimli::DW_AT_decl_file)? {
            AttributeValue::FileIndex(i) => i,
            other => other.udata_value()?,
        };
        let header = unit.line_program.as_ref()?.header();
        let file = header.file(index)?;
        // Line-table strings may sit in `.debug_line_str`, which only
        // `Dwarf::attr_string` reads
        let text = |attr| {
            self.dwarf
                .attr_string(unit, attr)
                .ok()
                .map(|s| s.to_string_lossy().into_owned())
        };
        let name = text(file.path_name())?;
        let path = match file.directory(header).and_then(text) {
            Some(dir) if !dir.is_empty() && !name.starts_with('/') => {
                format!("{}/{name}", dir.trim_end_matches('/'))
            }
            _ => name,
        };
        Some(SourceLocation { file: path, line })
    }

    pub(super) fn get_linkage_name(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<String> {
        // Try DW_AT_linkage_name first, then DW_AT_MIPS_linkage_name
        let attr = entry
            .attr_value(gimli::DW_AT_linkage_name)
            .or_else(|| entry.attr_value(gimli::DW_AT_MIPS_linkage_name));
        attr.and_then(|a| self.attr_to_string(unit, &a))
    }

    pub(super) fn attr_to_string(
        &self,
        unit: &Unit<Reader<'a>>,
        attr: &AttributeValue<Reader<'a>>,
    ) -> Option<String> {
        match attr {
            AttributeValue::DebugStrRef(offset) => self
                .dwarf
                .debug_str
                .get_str(*offset)
                .ok()
                .map(|s| s.to_string_lossy().to_string()),
            AttributeValue::String(s) => Some(s.to_string_lossy().to_string()),
            AttributeValue::DebugStrOffsetsIndex(_index) => self
                .dwarf
                .attr_string(unit, *attr)
                .ok()
                .map(|s| s.to_string_lossy().to_string()),
            _ => None,
        }
    }

    pub(super) fn get_byte_size(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<u64> {
        match entry.attr_value(gimli::DW_AT_byte_size)? {
            AttributeValue::Udata(size) => Some(size),
            AttributeValue::Data1(size) => Some(size as u64),
            AttributeValue::Data2(size) => Some(size as u64),
            AttributeValue::Data4(size) => Some(size as u64),
            AttributeValue::Data8(size) => Some(size),
            _ => None,
        }
    }

    pub(super) fn get_encoding(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<gimli::DwAte> {
        match entry.attr_value(gimli::DW_AT_encoding)? {
            AttributeValue::Encoding(enc) => Some(enc),
            _ => None,
        }
    }

    /// Register a type under its global .debug_info offset as an alias.
    /// This allows lookups via DebugInfoRef to find types that were parsed
    /// with unit-local keys.
    pub(super) fn register_global_alias(
        &mut self,
        local_key: DwarfTypeKey,
        global_offset: Option<usize>,
    ) {
        if let Some(global_off) = global_offset {
            // Get the TypeId that was registered for the local key
            if let Some(type_id) = self.type_table.get_by_dwarf_key(local_key) {
                // Register the same TypeId under the global key (using usize::MAX as sentinel)
                let global_key = GlobalTypeKey::global(global_off).to_dwarf_key();
                self.type_table.register_alias(global_key, type_id);
            }
        }
    }

    /// Get a type reference from DW_AT_type attribute.
    /// Returns a GlobalTypeKey that can handle both unit-local refs (UnitRef) and
    /// global refs (DebugInfoRef) which ARM compilers commonly use.
    pub(super) fn get_type_ref_key(
        &self,
        unit_index: usize,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<GlobalTypeKey> {
        match entry.attr_value(gimli::DW_AT_type)? {
            // Unit-local reference (common in GCC)
            AttributeValue::UnitRef(offset) => Some(GlobalTypeKey::unit_local(
                unit_index,
                offset.0.into_u64() as usize,
            )),
            // Global reference in .debug_info (common in ARM compilers)
            AttributeValue::DebugInfoRef(offset) => {
                Some(GlobalTypeKey::global(offset.0.into_u64() as usize))
            }
            _ => None,
        }
    }

    /// Legacy helper - get type ref as UnitOffset (for backwards compatibility where needed)
    pub(super) fn get_type_ref(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<UnitOffset> {
        match entry.attr_value(gimli::DW_AT_type)? {
            AttributeValue::UnitRef(offset) => Some(offset),
            _ => None,
        }
    }

    pub(super) fn get_member_offset(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<u64> {
        match entry.attr_value(gimli::DW_AT_data_member_location)? {
            // Direct offset values (common in DWARF 4+, GCC)
            AttributeValue::Udata(offset) => Some(offset),
            AttributeValue::Data1(offset) => Some(offset as u64),
            AttributeValue::Data2(offset) => Some(offset as u64),
            AttributeValue::Data4(offset) => Some(offset as u64),
            AttributeValue::Data8(offset) => Some(offset),
            AttributeValue::Sdata(offset) => Some(offset as u64),
            // Expression-based offset (common in DWARF 3, ARM Compiler 5)
            // ARM Compiler uses DW_OP_plus_uconst expressions like "23 N" meaning offset N
            AttributeValue::Exprloc(expr) => self.evaluate_member_offset_expr(unit, &expr),
            // Block form (older DWARF)
            AttributeValue::Block(block) => {
                let expr = gimli::Expression(block);
                self.evaluate_member_offset_expr(unit, &expr)
            }
            _ => Some(0),
        }
    }

    /// Evaluate a member offset expression (typically DW_OP_plus_uconst)
    pub(super) fn evaluate_member_offset_expr(
        &self,
        unit: &Unit<Reader<'a>>,
        expr: &gimli::Expression<Reader<'a>>,
    ) -> Option<u64> {
        let mut ops = (*expr).operations(unit.encoding());

        // Look for DW_OP_plus_uconst which is the common case for member offsets
        // The expression is evaluated with an implicit "base address" on the stack,
        // and DW_OP_plus_uconst adds the offset to it.
        while let Ok(Some(op)) = ops.next() {
            match op {
                // DW_OP_plus_uconst: adds a constant to top of stack
                // For member offsets, this IS the offset value
                gimli::Operation::PlusConstant { value } => {
                    return Some(value);
                }
                // DW_OP_constu followed by DW_OP_plus
                gimli::Operation::UnsignedConstant { value } => {
                    // Check if next op is Plus
                    if let Ok(Some(gimli::Operation::Plus)) = ops.next() {
                        return Some(value);
                    }
                    // Otherwise this might be the offset directly
                    return Some(value);
                }
                // Direct address/literal (less common but possible)
                gimli::Operation::Address { address } => {
                    return Some(address);
                }
                _ => continue,
            }
        }

        // Default to 0 if we can't parse the expression
        Some(0)
    }

    pub(super) fn get_bit_offset(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<u64> {
        // Try DW_AT_data_bit_offset first (DWARF 4+)
        if let Some(attr) = entry.attr_value(gimli::DW_AT_data_bit_offset) {
            return match attr {
                AttributeValue::Udata(v) => Some(v),
                AttributeValue::Data1(v) => Some(v as u64),
                AttributeValue::Data2(v) => Some(v as u64),
                AttributeValue::Data4(v) => Some(v as u64),
                AttributeValue::Data8(v) => Some(v),
                _ => None,
            };
        }

        // Fall back to DW_AT_bit_offset (DWARF 3)
        match entry.attr_value(gimli::DW_AT_bit_offset)? {
            AttributeValue::Udata(v) => Some(v),
            AttributeValue::Data1(v) => Some(v as u64),
            AttributeValue::Data2(v) => Some(v as u64),
            AttributeValue::Data4(v) => Some(v as u64),
            AttributeValue::Data8(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn get_bit_size(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<u64> {
        match entry.attr_value(gimli::DW_AT_bit_size)? {
            AttributeValue::Udata(v) => Some(v),
            AttributeValue::Data1(v) => Some(v as u64),
            AttributeValue::Data2(v) => Some(v as u64),
            AttributeValue::Data4(v) => Some(v as u64),
            AttributeValue::Data8(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn get_const_value(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<i64> {
        match entry.attr_value(gimli::DW_AT_const_value)? {
            AttributeValue::Sdata(v) => Some(v),
            AttributeValue::Udata(v) => Some(v as i64),
            AttributeValue::Data1(v) => Some(v as i64),
            AttributeValue::Data2(v) => Some(v as i64),
            AttributeValue::Data4(v) => Some(v as i64),
            AttributeValue::Data8(v) => Some(v as i64),
            _ => None,
        }
    }

    pub(super) fn get_virtuality(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<gimli::DwVirtuality> {
        match entry.attr_value(gimli::DW_AT_virtuality)? {
            AttributeValue::Virtuality(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn has_declaration_attr(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> bool {
        matches!(
            entry.attr_value(gimli::DW_AT_declaration),
            Some(AttributeValue::Flag(true))
        )
    }

    pub(super) fn has_external_attr(&self, entry: &DebuggingInformationEntry<Reader<'a>>) -> bool {
        matches!(
            entry.attr_value(gimli::DW_AT_external),
            Some(AttributeValue::Flag(true))
        )
    }

    pub(super) fn has_enum_class_attr(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> bool {
        matches!(
            entry.attr_value(gimli::DW_AT_enum_class),
            Some(AttributeValue::Flag(true))
        )
    }

    pub(super) fn has_artificial_attr(
        &self,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> bool {
        matches!(
            entry.attr_value(gimli::DW_AT_artificial),
            Some(AttributeValue::Flag(true))
        )
    }

    pub(super) fn get_array_count(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<u64> {
        // Look for DW_TAG_subrange_type child with count or upper_bound
        if let Ok(mut tree) = unit.entries_tree(Some(entry.offset()))
            && let Ok(root) = tree.root()
        {
            let mut children = root.children();
            while let Ok(Some(child)) = children.next() {
                if child.entry().tag() == gimli::DW_TAG_subrange_type {
                    // Try DW_AT_count first
                    if let Some(attr) = child.entry().attr_value(gimli::DW_AT_count) {
                        return match attr {
                            AttributeValue::Udata(v) => Some(v),
                            AttributeValue::Data1(v) => Some(v as u64),
                            AttributeValue::Data2(v) => Some(v as u64),
                            AttributeValue::Data4(v) => Some(v as u64),
                            AttributeValue::Data8(v) => Some(v),
                            _ => None,
                        };
                    }
                    // Try DW_AT_upper_bound (count = upper_bound + 1)
                    if let Some(attr) = child.entry().attr_value(gimli::DW_AT_upper_bound) {
                        return upper_bound_count(attr);
                    }
                }
            }
        }
        None
    }
}

/// Array element count from a `DW_AT_upper_bound` value. Compilers encode
/// zero-length and unbounded arrays with a bound of -1, which has no count. In
/// a 4- or 8-byte form that reads as all ones; in a 1- or 2-byte form all ones
/// is a real bound, such as 255 for `char buf[256]`.
fn upper_bound_count(attr: AttributeValue<Reader<'_>>) -> Option<u64> {
    let bound = match attr {
        AttributeValue::Data4(u32::MAX) => return None,
        AttributeValue::Udata(v) | AttributeValue::Data8(v) => v,
        AttributeValue::Data1(v) => u64::from(v),
        AttributeValue::Data2(v) => u64::from(v),
        AttributeValue::Data4(v) => u64::from(v),
        AttributeValue::Sdata(v) => u64::try_from(v).ok()?,
        _ => return None,
    };
    bound.checked_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub(super) fn upper_bound_of_minus_one_has_no_count() {
        let count = upper_bound_count;
        assert_eq!(count(AttributeValue::Data8(u64::MAX)), None);
        assert_eq!(count(AttributeValue::Udata(u64::MAX)), None);
        assert_eq!(count(AttributeValue::Data4(u32::MAX)), None);
        // A 1- or 2-byte bound of all ones is a real 256- or 65,536-element array.
        assert_eq!(count(AttributeValue::Data2(u16::MAX)), Some(65_536));
        assert_eq!(count(AttributeValue::Data1(u8::MAX)), Some(256));
        assert_eq!(count(AttributeValue::Sdata(-1)), None);
        assert_eq!(count(AttributeValue::Sdata(-7)), None);
    }

    #[test]
    pub(super) fn upper_bound_adds_one() {
        assert_eq!(upper_bound_count(AttributeValue::Data1(9)), Some(10));
        assert_eq!(upper_bound_count(AttributeValue::Data4(0)), Some(1));
        assert_eq!(upper_bound_count(AttributeValue::Sdata(3)), Some(4));
        assert_eq!(upper_bound_count(AttributeValue::Data8(99)), Some(100));
    }
}
