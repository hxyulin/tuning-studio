//! Variable DIEs, the cache that resolves `abstract_origin` and `specification`, and the deferred pass.

use super::status::VariableStatus;
use super::{DwarfParser, Reader};
use crate::type_table::*;
use gimli::{AttributeValue, DebuggingInformationEntry, ReaderOffset, Unit};

#[derive(Debug, Clone)]
pub(super) struct VariableInfo {
    pub(super) name: Option<String>,
    pub(super) linkage_name: Option<String>,
    pub(super) type_key: Option<DwarfTypeKey>,
    pub(super) size: u64,
    pub(super) is_global: bool,
}

impl VariableInfo {
    /// Inherit missing fields from another VariableInfo (e.g., from abstract_origin target)
    pub(super) fn inherit_from(&mut self, other: &VariableInfo) {
        if self.name.is_none() {
            self.name = other.name.clone();
        }
        if self.linkage_name.is_none() {
            self.linkage_name = other.linkage_name.clone();
        }
        if self.type_key.is_none() {
            self.type_key = other.type_key;
        }
        if self.size == 0 {
            self.size = other.size;
        }
        if !self.is_global {
            self.is_global = other.is_global;
        }
    }
}

/// A variable needing deferred resolution (target DIE appeared before this DIE)
#[derive(Debug)]
pub(super) struct DeferredVariable {
    pub(super) info: VariableInfo,
    pub(super) status: VariableStatus,
    pub(super) target_offset: usize,
}

/// A symbol pending type resolution
#[derive(Debug)]
pub(super) struct PendingSymbol {
    pub(super) name: String,
    pub(super) mangled_name: Option<String>,
    pub(super) size: u64,
    pub(super) type_key: Option<DwarfTypeKey>,
    pub(super) is_global: bool,
    pub(super) status: VariableStatus,
}
impl<'a> DwarfParser<'a, Reader<'a>> {
    /// Parse DW_TAG_variable
    pub(super) fn parse_variable(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        _key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let global_offset = self.get_global_offset(unit, entry);

        // Extract variable info from this DIE
        let mut info = VariableInfo {
            name: self.get_name(unit, entry),
            linkage_name: self.get_linkage_name(unit, entry),
            type_key: self
                .get_type_ref_key(unit_index, entry)
                .map(|k| k.to_dwarf_key()),
            size: self.get_byte_size(entry).unwrap_or(0),
            is_global: self.has_external_attr(entry),
        };

        // Cache variables with type info for abstract_origin/specification resolution
        if info.type_key.is_some()
            && let Some(off) = global_offset
        {
            self.variable_cache.insert(off, info.clone());
        }

        // Pure declarations shouldn't become symbols - they're referenced via specification
        if self.has_declaration_attr(entry) {
            return;
        }

        // Get variable status (includes address if available)
        let status = self.get_variable_status(unit, entry);

        // Check for DW_AT_specification or DW_AT_abstract_origin and inherit info
        let target_offset = self.get_reference_offset(unit, entry);
        if let Some(target_off) = target_offset {
            if let Some(target_info) = self.variable_cache.get(&target_off) {
                info.inherit_from(target_info);
            } else {
                // Target not yet parsed - defer to second pass
                self.deferred_variables.push(DeferredVariable {
                    info,
                    status,
                    target_offset: target_off,
                });
                return;
            }
        }

        // Create pending symbol if we have enough info
        self.try_add_pending_symbol(info, status);
    }

    /// Get the global .debug_info offset for a DIE
    pub(super) fn get_global_offset(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<usize> {
        entry
            .offset()
            .to_debug_info_offset(&unit.header)
            .map(|o| o.0.into_u64() as usize)
    }

    /// Get the target offset from DW_AT_specification or DW_AT_abstract_origin
    pub(super) fn get_reference_offset(
        &self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<usize> {
        // Try DW_AT_specification first, then DW_AT_abstract_origin
        for attr_name in [gimli::DW_AT_specification, gimli::DW_AT_abstract_origin] {
            if let Some(attr) = entry.attr_value(attr_name) {
                let offset = match attr {
                    AttributeValue::UnitRef(offset) => offset
                        .to_debug_info_offset(&unit.header)
                        .map(|o| o.0.into_u64() as usize),
                    AttributeValue::DebugInfoRef(offset) => Some(offset.0.into_u64() as usize),
                    _ => None,
                };
                if offset.is_some() {
                    return offset;
                }
            }
        }
        None
    }

    /// Try to create a pending symbol from variable info
    pub(super) fn try_add_pending_symbol(&mut self, info: VariableInfo, status: VariableStatus) {
        let name = info.linkage_name.clone().or(info.name.clone());
        let name = match name {
            Some(n) if !n.is_empty() => n,
            _ => return, // Skip anonymous variables
        };

        if info.type_key.is_none() {
            return; // Skip variables without type info
        }

        self.pending_symbols.push(PendingSymbol {
            name: info.name.unwrap_or_else(|| name.clone()),
            mangled_name: if info.linkage_name.as_ref() != Some(&name) {
                info.linkage_name
            } else {
                None
            },
            size: info.size,
            type_key: info.type_key,
            is_global: info.is_global,
            status,
        });
    }

    /// Resolve deferred variables that couldn't be resolved on first pass
    pub(super) fn resolve_pending_specifications(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_variables);

        for var in deferred {
            let mut info = var.info;

            // Try to find the target now
            if let Some(target_info) = self.variable_cache.get(&var.target_offset) {
                info.inherit_from(target_info);
            }

            self.try_add_pending_symbol(info, var.status);
        }
    }
}
