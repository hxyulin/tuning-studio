//! Handlers for the type DIEs: base types, structs, enums and the like, plus the passes that finish them.

use super::{DwarfParser, Reader};
use crate::types::*;
use gimli::{AttributeValue, DebuggingInformationEntry, ReaderOffset, Unit, UnitOffset};

impl<'a> DwarfParser<'a, Reader<'a>> {
    /// Parse DW_TAG_base_type
    pub(super) fn parse_base_type(
        &mut self,
        unit: &Unit<Reader<'a>>,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let name = self.get_name(unit, entry).unwrap_or_default();
        let size = self.get_byte_size(entry).unwrap_or(0);
        let encoding = self.get_encoding(entry);

        let prim = match encoding {
            Some(gimli::DW_ATE_boolean) => PrimitiveDef::Bool,
            Some(gimli::DW_ATE_signed_char) => PrimitiveDef::SignedChar,
            Some(gimli::DW_ATE_unsigned_char) => PrimitiveDef::UnsignedChar,
            Some(gimli::DW_ATE_signed) => match size {
                1 => PrimitiveDef::SignedChar,
                2 => PrimitiveDef::Short,
                4 => PrimitiveDef::Int,
                8 => PrimitiveDef::LongLong,
                _ => PrimitiveDef::SizedInt { size, signed: true },
            },
            Some(gimli::DW_ATE_unsigned) => match size {
                1 => PrimitiveDef::UnsignedChar,
                2 => PrimitiveDef::UnsignedShort,
                4 => PrimitiveDef::UnsignedInt,
                8 => PrimitiveDef::UnsignedLongLong,
                _ => PrimitiveDef::SizedInt {
                    size,
                    signed: false,
                },
            },
            Some(gimli::DW_ATE_float) => match size {
                4 => PrimitiveDef::Float,
                8 => PrimitiveDef::Double,
                16 => PrimitiveDef::LongDouble,
                _ => PrimitiveDef::SizedFloat { size },
            },
            Some(gimli::DW_ATE_UTF) => PrimitiveDef::UnicodeChar { size },
            _ => {
                // Try to infer from name
                match name.as_str() {
                    "bool" | "_Bool" => PrimitiveDef::Bool,
                    "char" => PrimitiveDef::Char,
                    "signed char" => PrimitiveDef::SignedChar,
                    "unsigned char" => PrimitiveDef::UnsignedChar,
                    "short" | "short int" => PrimitiveDef::Short,
                    "unsigned short" | "short unsigned int" => PrimitiveDef::UnsignedShort,
                    "int" => PrimitiveDef::Int,
                    "unsigned int" | "unsigned" => PrimitiveDef::UnsignedInt,
                    "long" | "long int" => PrimitiveDef::Long,
                    "unsigned long" | "long unsigned int" => PrimitiveDef::UnsignedLong,
                    "long long" | "long long int" => PrimitiveDef::LongLong,
                    "unsigned long long" | "long long unsigned int" => {
                        PrimitiveDef::UnsignedLongLong
                    }
                    "float" => PrimitiveDef::Float,
                    "double" => PrimitiveDef::Double,
                    "long double" => PrimitiveDef::LongDouble,
                    _ => PrimitiveDef::SizedInt { size, signed: true },
                }
            }
        };

        self.type_table
            .insert_for_key(key, TypeDef::Primitive(prim));
    }

    /// Parse DW_TAG_pointer_type
    pub(super) fn parse_pointer_type(
        &mut self,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let inner_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            // Void pointer
            self.ensure_void_type()
        };

        self.type_table
            .insert_for_key(key, TypeDef::Pointer(inner_id));
    }

    /// Parse DW_TAG_reference_type
    pub(super) fn parse_reference_type(
        &mut self,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let inner_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            self.ensure_void_type()
        };

        self.type_table
            .insert_for_key(key, TypeDef::Reference(inner_id));
    }

    /// Parse DW_TAG_const_type
    pub(super) fn parse_const_type(
        &mut self,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let inner_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            self.ensure_void_type()
        };

        self.type_table
            .insert_for_key(key, TypeDef::Const(inner_id));
    }

    /// Parse DW_TAG_volatile_type
    pub(super) fn parse_volatile_type(
        &mut self,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let inner_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            self.ensure_void_type()
        };

        self.type_table
            .insert_for_key(key, TypeDef::Volatile(inner_id));
    }

    /// Parse DW_TAG_restrict_type
    pub(super) fn parse_restrict_type(
        &mut self,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let inner_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            self.ensure_void_type()
        };

        self.type_table
            .insert_for_key(key, TypeDef::Restrict(inner_id));
    }

    /// Parse DW_TAG_typedef
    pub(super) fn parse_typedef(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let name = self.get_name(unit, entry).unwrap_or_default();

        let underlying = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            self.ensure_void_type()
        };

        self.type_table
            .insert_for_key(key, TypeDef::Typedef { name, underlying });
    }

    /// Parse DW_TAG_array_type
    pub(super) fn parse_array_type(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let element_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            self.ensure_void_type()
        };

        // Get array count from subrange child
        let count = self.get_array_count(unit, entry);

        self.type_table.insert_for_key(
            key,
            TypeDef::Array {
                element: element_id,
                count,
            },
        );
    }

    /// Parse DW_TAG_structure_type, DW_TAG_class_type, or DW_TAG_union_type
    pub(super) fn parse_struct_type(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
        is_union: bool,
    ) {
        let name = self.get_name(unit, entry);
        let size = self.get_byte_size(entry).unwrap_or(0);
        let is_class = entry.tag() == gimli::DW_TAG_class_type;
        let is_declaration = self.has_declaration_attr(entry);

        // Check if this is a forward declaration
        if is_declaration {
            let kind = if is_union {
                ForwardDeclKind::Union
            } else if is_class {
                ForwardDeclKind::Class
            } else {
                ForwardDeclKind::Struct
            };

            self.type_table.insert_for_key(
                key,
                TypeDef::ForwardDecl {
                    name: name.unwrap_or_default(),
                    kind,
                    target: None,
                },
            );
            return;
        }

        // Pre-allocate the TypeId so members can reference it (for recursive types)
        let struct_id = self.type_table.get_or_allocate(key);

        let mut struct_def = StructDef::new(name, size, is_class);

        // Parse children (members, base classes, template params)
        if let Ok(mut tree) = unit.entries_tree(Some(entry.offset()))
            && let Ok(root) = tree.root()
        {
            let mut children = root.children();
            while let Ok(Some(child)) = children.next() {
                let child_entry = child.entry();
                match child_entry.tag() {
                    gimli::DW_TAG_member => {
                        if let Some(member) = self.parse_member(unit, unit_index, child_entry) {
                            struct_def.members.push(member);
                        }
                    }
                    gimli::DW_TAG_inheritance => {
                        if let Some(base) = self.parse_inheritance(unit, unit_index, child_entry) {
                            struct_def.base_classes.push(base);
                        }
                    }
                    gimli::DW_TAG_template_type_parameter => {
                        if let Some(param) =
                            self.parse_template_type_param(unit, unit_index, child_entry)
                        {
                            struct_def.template_params.push(param);
                        }
                    }
                    gimli::DW_TAG_template_value_parameter => {
                        if let Some(param) = self.parse_template_value_param(unit, child_entry) {
                            struct_def.template_params.push(param);
                        }
                    }
                    gimli::DW_TAG_variant_part => {
                        struct_def.variant_part =
                            Some(self.parse_variant_part(unit, unit_index, child));
                    }
                    _ => {}
                }
            }
        }

        let def = if is_union {
            TypeDef::Union(struct_def)
        } else {
            TypeDef::Struct(struct_def)
        };

        self.type_table.define(struct_id, def);
    }

    /// Parse a Rust enum's DW_TAG_variant_part: one discriminant member plus
    /// DW_TAG_variant children, each holding a single member for its payload.
    pub(super) fn parse_variant_part(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        node: gimli::EntriesTreeNode<Reader<'a>>,
    ) -> VariantPart {
        let discr_ref = match node.entry().attr_value(gimli::DW_AT_discr) {
            Some(AttributeValue::UnitRef(offset)) => Some(offset),
            _ => None,
        };

        let mut part = VariantPart::default();
        let mut members: Vec<(UnitOffset, MemberDef)> = Vec::new();
        let mut children = node.children();
        while let Ok(Some(child)) = children.next() {
            let entry = child.entry();
            match entry.tag() {
                gimli::DW_TAG_member => {
                    let offset = entry.offset();
                    if let Some(member) = self.parse_member(unit, unit_index, entry) {
                        members.push((offset, member));
                    }
                }
                gimli::DW_TAG_variant => {
                    let discr_value = entry.attr_value(gimli::DW_AT_discr_value).and_then(|v| {
                        v.udata_value()
                            .or_else(|| v.sdata_value().map(|s| s as u64))
                    });
                    let mut grandchildren = child.children();
                    while let Ok(Some(gc)) = grandchildren.next() {
                        if gc.entry().tag() == gimli::DW_TAG_member {
                            if let Some(member) = self.parse_member(unit, unit_index, gc.entry()) {
                                part.variants.push(VariantDef {
                                    discr_value,
                                    member,
                                    location: self.decl_location(unit, gc.entry()),
                                });
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        part.discriminant = match discr_ref {
            Some(target) => members
                .into_iter()
                .find(|(o, _)| *o == target)
                .map(|(_, m)| m),
            None => members.into_iter().next().map(|(_, m)| m),
        };
        part
    }

    /// Parse DW_TAG_member
    pub(super) fn parse_member(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<MemberDef> {
        let name = self.get_name(unit, entry).unwrap_or_default();
        let offset = self.get_member_offset(unit, entry).unwrap_or(0);

        let type_id = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            self.type_table.get_or_allocate(ref_key.to_dwarf_key())
        } else {
            TypeId::INVALID
        };

        let mut member = MemberDef::new(name, offset, type_id);
        member.bit_offset = self.get_bit_offset(entry);
        member.bit_size = self.get_bit_size(entry);

        Some(member)
    }

    /// Parse DW_TAG_inheritance
    pub(super) fn parse_inheritance(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<BaseClassDef> {
        let ref_key = self.get_type_ref_key(unit_index, entry)?;
        let base_type_id = self.type_table.get_or_allocate(ref_key.to_dwarf_key());

        let offset = self.get_member_offset(unit, entry).unwrap_or(0);
        let is_virtual = self.get_virtuality(entry).is_some();

        Some(BaseClassDef {
            type_id: base_type_id,
            offset,
            is_virtual,
        })
    }

    /// Parse DW_TAG_template_type_parameter
    pub(super) fn parse_template_type_param(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<TemplateParam> {
        let name = self
            .get_name(unit, entry)
            .unwrap_or_else(|| "T".to_string());

        let type_id = if let Some(type_offset) = self.get_type_ref(entry) {
            let ref_key = DwarfTypeKey::new(unit_index, type_offset.0.into_u64() as usize);
            self.type_table.get_or_allocate(ref_key)
        } else {
            self.ensure_void_type()
        };

        Some(TemplateParam::Type { name, type_id })
    }

    /// Parse DW_TAG_template_value_parameter
    pub(super) fn parse_template_value_param(
        &mut self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<TemplateParam> {
        let name = self
            .get_name(unit, entry)
            .unwrap_or_else(|| "N".to_string());
        let value = self.get_const_value(entry).unwrap_or(0);

        Some(TemplateParam::Value { name, value })
    }

    /// Parse DW_TAG_enumeration_type
    pub(super) fn parse_enum_type(
        &mut self,
        unit: &Unit<Reader<'a>>,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let name = self.get_name(unit, entry);
        let size = self.get_byte_size(entry).unwrap_or(4);
        let is_declaration = self.has_declaration_attr(entry);

        // Check if this is a forward declaration
        if is_declaration {
            self.type_table.insert_for_key(
                key,
                TypeDef::ForwardDecl {
                    name: name.unwrap_or_default(),
                    kind: ForwardDeclKind::Enum,
                    target: None,
                },
            );
            return;
        }

        // Check for C++11 enum class (DW_AT_enum_class)
        let is_scoped = self.has_enum_class_attr(entry);

        // Get underlying type if specified
        let underlying_type = if let Some(type_offset) = self.get_type_ref(entry) {
            let ref_key = DwarfTypeKey::new(unit_index, type_offset.0.into_u64() as usize);
            Some(self.type_table.get_or_allocate(ref_key))
        } else {
            None
        };

        let mut enum_def = EnumDef::new(name, size, is_scoped);
        enum_def.underlying_type = underlying_type;

        // Parse enumerator children
        if let Ok(mut tree) = unit.entries_tree(Some(entry.offset()))
            && let Ok(root) = tree.root()
        {
            let mut children = root.children();
            while let Ok(Some(child)) = children.next() {
                if child.entry().tag() == gimli::DW_TAG_enumerator
                    && let Some(variant) = self.parse_enumerator(unit, child.entry())
                {
                    enum_def.variants.push(variant);
                }
            }
        }

        self.type_table.insert_for_key(key, TypeDef::Enum(enum_def));
    }

    /// Parse DW_TAG_enumerator
    pub(super) fn parse_enumerator(
        &mut self,
        unit: &Unit<Reader<'a>>,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) -> Option<EnumVariant> {
        let name = self.get_name(unit, entry)?;
        let value = self.get_const_value(entry).unwrap_or(0);
        Some(EnumVariant { name, value })
    }

    /// Parse DW_TAG_subroutine_type
    pub(super) fn parse_subroutine_type(
        &mut self,
        unit_index: usize,
        key: DwarfTypeKey,
        entry: &DebuggingInformationEntry<Reader<'a>>,
    ) {
        let return_type = if let Some(ref_key) = self.get_type_ref_key(unit_index, entry) {
            Some(self.type_table.get_or_allocate(ref_key.to_dwarf_key()))
        } else {
            None // void return
        };

        // Note: We could parse parameter types from children, but for now keep it simple
        self.type_table.insert_for_key(
            key,
            TypeDef::Subroutine {
                return_type,
                params: Vec::new(),
            },
        );
    }

    /// Resolve inherited members from base classes
    pub(super) fn resolve_inherited_members(&mut self) {
        // Collect all struct/union type IDs
        let struct_ids: Vec<TypeId> = self
            .type_table
            .all_type_ids()
            .filter(|&id| {
                matches!(
                    self.type_table.get(id),
                    Some(TypeDef::Struct(_)) | Some(TypeDef::Union(_))
                )
            })
            .collect();

        for id in struct_ids {
            self.resolve_inherited_members_for(id, &mut std::collections::HashSet::new());
        }
    }

    pub(super) fn resolve_inherited_members_for(
        &mut self,
        id: TypeId,
        visited: &mut std::collections::HashSet<TypeId>,
    ) {
        if visited.contains(&id) {
            return; // Prevent infinite recursion
        }
        visited.insert(id);

        // Get base classes for this struct
        let base_classes: Vec<BaseClassDef> = match self.type_table.get(id) {
            Some(TypeDef::Struct(s)) | Some(TypeDef::Union(s)) => s.base_classes.clone(),
            _ => return,
        };

        if base_classes.is_empty() {
            return;
        }

        // Collect members from base classes
        let mut inherited_members = Vec::new();
        for base in &base_classes {
            // First resolve the base class's inherited members
            let resolved_base_id = self.type_table.resolve(base.type_id);
            self.resolve_inherited_members_for(resolved_base_id, visited);

            // Then collect its members
            if let Some(members) = self.type_table.members(resolved_base_id) {
                for member in members {
                    inherited_members.push(MemberDef {
                        name: member.name.clone(),
                        offset: member.offset.saturating_add(base.offset),
                        type_id: member.type_id,
                        bit_offset: member.bit_offset,
                        bit_size: member.bit_size,
                    });
                }
            }
        }

        // Add inherited members to the struct
        if !inherited_members.is_empty()
            && let Some(TypeDef::Struct(s)) | Some(TypeDef::Union(s)) = self.type_table.get_mut(id)
        {
            // Insert inherited members at the beginning
            inherited_members.append(&mut s.members);
            s.members = inherited_members;
        }
    }

    /// Ensure we have a void type and return its ID
    pub(super) fn ensure_void_type(&mut self) -> TypeId {
        // Look for an existing void type
        for (id, def) in self.type_table.iter() {
            if matches!(def, TypeDef::Void) {
                return id;
            }
        }
        // Create one if not found
        self.type_table.insert(TypeDef::Void)
    }
}
