//! Global Type Table for DWARF Type Resolution
//!
//! This module provides a centralized type table where types are stored by index (TypeId).
//! All type references use TypeId instead of `Box<TypeInfo>`, which simplifies:
//! - Forward declaration resolution (just update the target)
//! - Template support (template instantiations share base type references)
//! - Memory efficiency (no deep nesting of boxed types)
//!
//! The TypeHandle struct wraps `Arc<TypeTable>` + TypeId for zero-overhead type access.

use crate::types::*;
use crate::variable_type::VariableType;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Longest typedef, qualifier or array chain followed before the debug info is treated as cyclic.
const MAX_TYPE_DEPTH: usize = 32;
/// The global type table
#[derive(Debug)]
pub struct TypeTable {
    /// All type definitions, indexed by TypeId
    types: Vec<TypeDef>,
    /// Maps DWARF DIE location to TypeId for deduplication
    dwarf_to_id: HashMap<DwarfTypeKey, TypeId>,
    /// Maps type names to their canonical TypeId (for forward declaration resolution)
    name_to_id: HashMap<String, Vec<TypeId>>,
    /// Pending forward declarations that need resolution
    pending_forward_decls: Vec<TypeId>,
    /// Types declared in Rust compile units; named with Rust syntax
    rust_types: HashSet<TypeId>,
    /// Compiler-provided names that the structural name cannot reproduce
    /// (Rust `u16`, `&mut T`, `fn() -> !`)
    source_names: HashMap<TypeId, String>,
}

impl Default for TypeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeTable {
    /// Create a new empty type table
    pub fn new() -> Self {
        Self {
            types: Vec::new(),
            dwarf_to_id: HashMap::new(),
            name_to_id: HashMap::new(),
            pending_forward_decls: Vec::new(),
            rust_types: HashSet::new(),
            source_names: HashMap::new(),
        }
    }

    /// Mark a type as coming from a Rust compile unit
    pub(crate) fn mark_rust(&mut self, id: TypeId) {
        self.rust_types.insert(id);
    }

    /// Whether a type was declared in a Rust compile unit
    pub fn is_rust(&self, id: TypeId) -> bool {
        self.rust_types.contains(&id)
    }

    /// Record the compiler's own spelling of a type name
    pub(crate) fn set_source_name(&mut self, id: TypeId, name: String) {
        self.source_names.insert(id, name);
    }

    /// Get the number of types in the table
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Check if the table is empty
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Get the number of DWARF key mappings (including global offset aliases)
    pub fn dwarf_key_count(&self) -> usize {
        self.dwarf_to_id.len()
    }

    /// Allocate a new type ID without setting its definition
    pub(crate) fn allocate(&mut self) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        self.types.push(TypeDef::Placeholder);
        id
    }

    /// Get or create a TypeId for a DWARF key
    pub(crate) fn get_or_allocate(&mut self, key: DwarfTypeKey) -> TypeId {
        if let Some(&id) = self.dwarf_to_id.get(&key) {
            return id;
        }
        let id = self.allocate();
        self.dwarf_to_id.insert(key, id);
        id
    }

    /// Get the TypeId for a DWARF key if it exists
    pub(crate) fn get_by_dwarf_key(&self, key: DwarfTypeKey) -> Option<TypeId> {
        self.dwarf_to_id.get(&key).copied()
    }

    /// Register an existing TypeId under an additional key.
    /// This is used to register the same type under both its unit-local key
    /// and its global .debug_info offset, so that DebugInfoRef lookups work.
    pub(crate) fn register_alias(&mut self, key: DwarfTypeKey, id: TypeId) {
        self.dwarf_to_id.entry(key).or_insert(id);
    }

    /// Give a still-empty `id` the definition of `from`. A reference by global
    /// offset allocates a type before the DIE it names is parsed under its
    /// unit-local key, and the alias registered then keeps the empty one.
    pub(crate) fn fill_placeholder(&mut self, id: TypeId, from: TypeId) {
        if id == from || !matches!(self.get(id), Some(TypeDef::Placeholder)) {
            return;
        }
        let def = self.types[from.0 as usize].clone();
        if def.is_forward_decl() {
            self.pending_forward_decls.push(id);
        }
        self.types[id.0 as usize] = def;
        if self.rust_types.contains(&from) {
            self.rust_types.insert(id);
        }
        if let Some(name) = self.source_names.get(&from).cloned() {
            self.source_names.insert(id, name);
        }
    }

    /// Define a type at the given TypeId
    pub(crate) fn define(&mut self, id: TypeId, def: TypeDef) {
        assert!(
            (id.0 as usize) < self.types.len(),
            "TypeId {} out of range",
            id.0
        );

        if def.is_forward_decl() {
            self.pending_forward_decls.push(id);
        }

        if let Some(name) = def.name()
            && !def.is_forward_decl()
        {
            self.name_to_id
                .entry(name.to_string())
                .or_default()
                .push(id);
        }

        self.types[id.0 as usize] = def;
    }

    /// Insert a new type and return its TypeId
    pub(crate) fn insert(&mut self, def: TypeDef) -> TypeId {
        let id = self.allocate();
        self.define(id, def);
        id
    }

    /// Insert a type for a DWARF key
    pub(crate) fn insert_for_key(&mut self, key: DwarfTypeKey, def: TypeDef) -> TypeId {
        let id = self.get_or_allocate(key);
        self.define(id, def);
        id
    }

    /// Get a type definition by TypeId
    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        if id == TypeId::INVALID {
            return None;
        }
        self.types.get(id.0 as usize)
    }

    /// Get a mutable reference to a type definition
    pub(crate) fn get_mut(&mut self, id: TypeId) -> Option<&mut TypeDef> {
        if id == TypeId::INVALID {
            return None;
        }
        self.types.get_mut(id.0 as usize)
    }

    /// Resolve a TypeId, following forward declarations to their targets
    pub fn resolve(&self, id: TypeId) -> TypeId {
        let mut id = id;
        for _ in 0..MAX_TYPE_DEPTH {
            match self.get(id) {
                Some(TypeDef::ForwardDecl {
                    target: Some(target),
                    ..
                }) => id = *target,
                _ => return id,
            }
        }
        // A forward declaration cycle has no definition to resolve to.
        id
    }

    /// Follow typedefs, qualifiers and resolved forward declarations to the
    /// type they wrap. Returns None if the chain is longer than
    /// `MAX_TYPE_DEPTH`, which only happens when the debug info has a cycle.
    fn strip(&self, mut id: TypeId) -> Option<TypeId> {
        for _ in 0..MAX_TYPE_DEPTH {
            match self.get(id)? {
                TypeDef::Typedef { underlying: n, .. }
                | TypeDef::Const(n)
                | TypeDef::Volatile(n)
                | TypeDef::Restrict(n)
                | TypeDef::ForwardDecl {
                    target: Some(n), ..
                } => id = *n,
                _ => return Some(id),
            }
        }
        None
    }

    /// Get the resolved type definition
    pub fn resolved(&self, id: TypeId) -> Option<&TypeDef> {
        self.get(self.resolve(id))
    }

    /// Find all TypeIds for types with the given name
    pub fn find_by_name(&self, name: &str) -> Vec<TypeId> {
        self.name_to_id.get(name).cloned().unwrap_or_default()
    }

    /// Find the best definition for a type name
    pub fn find_best_definition(&self, name: &str, kind: ForwardDeclKind) -> Option<TypeId> {
        let candidates = self.find_by_name(name);
        let mut best: Option<(TypeId, usize)> = None;

        for &id in &candidates {
            if let Some(def) = self.get(id) {
                let (matches_kind, member_count) = match (kind, def) {
                    (ForwardDeclKind::Struct | ForwardDeclKind::Class, TypeDef::Struct(s)) => {
                        (true, s.members.len())
                    }
                    (ForwardDeclKind::Union, TypeDef::Union(s)) => (true, s.members.len()),
                    (ForwardDeclKind::Enum, TypeDef::Enum(e)) => (true, e.variants.len()),
                    _ => (false, 0),
                };

                if matches_kind && best.is_none_or(|(_, best_count)| member_count > best_count) {
                    best = Some((id, member_count));
                }
            }
        }

        best.map(|(id, _)| id)
    }

    /// Resolve all pending forward declarations
    pub(crate) fn resolve_forward_declarations(&mut self) {
        let pending = std::mem::take(&mut self.pending_forward_decls);
        let mut resolved_count = 0;

        for id in pending {
            if let Some(TypeDef::ForwardDecl { name, kind, target }) = self.get(id).cloned() {
                if target.is_some() {
                    continue;
                }

                if let Some(best_id) = self.find_best_definition(&name, kind)
                    && best_id != id
                    && let Some(TypeDef::ForwardDecl { target, .. }) = self.get_mut(id)
                {
                    *target = Some(best_id);
                    resolved_count += 1;
                }
            }
        }

        tracing::debug!("Resolved {} forward declarations", resolved_count);
    }

    /// Get all type IDs in the table
    pub fn all_type_ids(&self) -> impl Iterator<Item = TypeId> {
        (0..self.types.len() as u32).map(TypeId)
    }

    /// Get the size of a type in bytes
    pub fn type_size(&self, id: TypeId) -> Option<u64> {
        self.type_size_with_depth(id, 0)
    }

    fn type_size_with_depth(&self, id: TypeId, depth: usize) -> Option<u64> {
        // Arrays recurse into their element, so a self-referencing array needs its own bound.
        if depth > MAX_TYPE_DEPTH {
            return None;
        }

        match self.get(self.strip(id)?)? {
            TypeDef::Primitive(p) => Some(p.size()),
            TypeDef::Pointer(_) | TypeDef::Reference(_) => Some(4),
            TypeDef::Array { element, count } => {
                let elem_size = self.type_size_with_depth(*element, depth + 1)?;
                elem_size.checked_mul(count.unwrap_or(0))
            }
            TypeDef::Struct(s) | TypeDef::Union(s) => Some(s.size),
            TypeDef::Enum(e) => Some(e.size),
            TypeDef::Subroutine { .. } => Some(4),
            TypeDef::Void => Some(0),
            TypeDef::Unknown { size } => Some(*size),
            // strip() only stops on these when they cannot be followed further.
            TypeDef::ForwardDecl { .. } | TypeDef::Placeholder => None,
            TypeDef::Typedef { .. }
            | TypeDef::Const(_)
            | TypeDef::Volatile(_)
            | TypeDef::Restrict(_) => None,
        }
    }

    /// Get the display name of a type
    pub fn type_name(&self, id: TypeId) -> String {
        self.type_name_with_depth(id, 0)
    }

    fn type_name_with_depth(&self, id: TypeId, depth: usize) -> String {
        if depth > 20 {
            return "<recursive>".to_string();
        }

        let resolved_id = self.resolve(id);
        if let Some(name) = self.source_names.get(&resolved_id) {
            return name.clone();
        }
        if self.is_rust(resolved_id)
            && let Some(name) = self.rust_type_name(resolved_id, depth)
        {
            return name;
        }
        match self.get(resolved_id) {
            None => format!("<invalid:{}>", id.0),
            Some(def) => match def {
                TypeDef::Primitive(p) => p.name().to_string(),
                TypeDef::Pointer(inner) => {
                    format!("{}*", self.type_name_with_depth(*inner, depth + 1))
                }
                TypeDef::Reference(inner) => {
                    format!("{}&", self.type_name_with_depth(*inner, depth + 1))
                }
                TypeDef::Array { element, count } => {
                    let elem_name = self.type_name_with_depth(*element, depth + 1);
                    match count {
                        Some(n) => format!("{}[{}]", elem_name, n),
                        None => format!("{}[]", elem_name),
                    }
                }
                TypeDef::Struct(s) => s.display_name(),
                TypeDef::Union(s) => format!("union {}", s.display_name()),
                TypeDef::Enum(e) => e.name.as_deref().unwrap_or("<anonymous enum>").to_string(),
                TypeDef::Typedef { name, .. } => name.clone(),
                TypeDef::Const(inner) => {
                    format!("const {}", self.type_name_with_depth(*inner, depth + 1))
                }
                TypeDef::Volatile(inner) => {
                    format!("volatile {}", self.type_name_with_depth(*inner, depth + 1))
                }
                TypeDef::Restrict(inner) => {
                    format!("restrict {}", self.type_name_with_depth(*inner, depth + 1))
                }
                TypeDef::Subroutine { return_type, .. } => {
                    let ret = return_type
                        .map(|r| self.type_name_with_depth(r, depth + 1))
                        .unwrap_or_else(|| "void".to_string());
                    format!("{}()", ret)
                }
                TypeDef::Void => "void".to_string(),
                TypeDef::Unknown { size } => format!("unknown[{}]", size),
                TypeDef::ForwardDecl { name, kind, target } => {
                    if let Some(target) = target {
                        self.type_name_with_depth(*target, depth + 1)
                    } else {
                        let prefix = match kind {
                            ForwardDeclKind::Struct => "struct",
                            ForwardDeclKind::Class => "class",
                            ForwardDeclKind::Union => "union",
                            ForwardDeclKind::Enum => "enum",
                        };
                        format!("{} {} (forward)", prefix, name)
                    }
                }
                TypeDef::Placeholder => "<placeholder>".to_string(),
            },
        }
    }

    /// Rust spellings for the structural types that differ from C
    fn rust_type_name(&self, id: TypeId, depth: usize) -> Option<String> {
        Some(match self.get(id)? {
            TypeDef::Array { element, count } => {
                let elem = self.type_name_with_depth(*element, depth + 1);
                match count {
                    Some(n) => format!("[{elem}; {n}]"),
                    None => format!("[{elem}]"),
                }
            }
            TypeDef::Pointer(inner) => {
                format!("*const {}", self.type_name_with_depth(*inner, depth + 1))
            }
            // rustc spells concrete arguments into the name; template params are
            // inherited by variant payload structs and would print `Some<T>`
            TypeDef::Struct(s) | TypeDef::Union(s) => s.display_name_without_params(),
            TypeDef::Void => "()".to_string(),
            _ => return None,
        })
    }

    /// Check if a type is expandable (has members that can be displayed)
    pub fn is_expandable(&self, id: TypeId) -> bool {
        self.is_expandable_with_depth(id, 0)
    }

    fn is_expandable_with_depth(&self, id: TypeId, depth: usize) -> bool {
        if depth > 20 {
            return false;
        }

        let resolved_id = self.resolve(id);
        match self.get(resolved_id) {
            None => false,
            Some(def) => match def {
                TypeDef::Struct(s) | TypeDef::Union(s) => {
                    !s.members.is_empty()
                        || s.variant_part
                            .as_ref()
                            .is_some_and(|v| !v.variants.is_empty())
                }
                // Arrays are expandable if they have a known count > 0
                TypeDef::Array { count, .. } => count.map(|c| c > 0).unwrap_or(false),
                // Pointers/references are expandable if they point to an expandable type
                TypeDef::Pointer(inner) | TypeDef::Reference(inner) => {
                    self.is_expandable_with_depth(*inner, depth + 1)
                }
                TypeDef::Typedef { underlying, .. } => {
                    self.is_expandable_with_depth(*underlying, depth + 1)
                }
                TypeDef::Const(inner) | TypeDef::Volatile(inner) | TypeDef::Restrict(inner) => {
                    self.is_expandable_with_depth(*inner, depth + 1)
                }
                TypeDef::ForwardDecl {
                    target: Some(target),
                    ..
                } => self.is_expandable_with_depth(*target, depth + 1),
                _ => false,
            },
        }
    }

    /// Check if a type can be added as a watchable variable
    pub fn is_addable(&self, id: TypeId) -> bool {
        let Some(stripped) = self.strip(id) else {
            return false;
        };
        match self.get(stripped) {
            Some(TypeDef::Primitive(_)) => true,
            Some(TypeDef::Enum(_)) => true,
            Some(TypeDef::Pointer(_)) | Some(TypeDef::Reference(_)) => true,
            Some(TypeDef::Struct(_)) | Some(TypeDef::Union(_)) | Some(TypeDef::Array { .. }) => {
                true
            }
            Some(TypeDef::Unknown { size }) => *size > 0,
            _ => false,
        }
    }

    /// Get the members of a type (if it's a struct/union)
    pub fn members(&self, id: TypeId) -> Option<&[MemberDef]> {
        self.members_with_depth(id, 0)
    }

    fn members_with_depth(&self, id: TypeId, depth: usize) -> Option<&[MemberDef]> {
        if depth > 20 {
            return None;
        }

        let resolved_id = self.resolve(id);
        match self.get(resolved_id)? {
            TypeDef::Struct(s) | TypeDef::Union(s) => Some(&s.members),
            TypeDef::Typedef { underlying, .. } => self.members_with_depth(*underlying, depth + 1),
            TypeDef::Const(inner) | TypeDef::Volatile(inner) | TypeDef::Restrict(inner) => {
                self.members_with_depth(*inner, depth + 1)
            }
            TypeDef::ForwardDecl {
                target: Some(target),
                ..
            } => self.members_with_depth(*target, depth + 1),
            _ => None,
        }
    }

    /// Get the underlying type for typedefs, const, volatile, etc.
    pub fn underlying(&self, id: TypeId) -> TypeId {
        self.underlying_with_depth(id, 0)
    }

    fn underlying_with_depth(&self, id: TypeId, depth: usize) -> TypeId {
        if depth > 20 {
            return id;
        }

        let resolved_id = self.resolve(id);
        match self.get(resolved_id) {
            Some(TypeDef::Typedef { underlying, .. })
            | Some(TypeDef::Const(underlying))
            | Some(TypeDef::Volatile(underlying))
            | Some(TypeDef::Restrict(underlying)) => {
                self.underlying_with_depth(*underlying, depth + 1)
            }
            _ => resolved_id,
        }
    }

    /// Convert a TypeId to VariableType
    pub fn to_variable_type(&self, id: TypeId) -> VariableType {
        let Some(stripped) = self.strip(id) else {
            return VariableType::Raw(0);
        };
        match self.get(stripped) {
            Some(TypeDef::Primitive(prim)) => prim.to_variable_type(),
            Some(TypeDef::Pointer(_)) | Some(TypeDef::Reference(_)) => VariableType::U32,
            Some(TypeDef::Enum(e)) => match e.size {
                1 => VariableType::U8,
                2 => VariableType::U16,
                4 => VariableType::U32,
                8 => VariableType::U64,
                _ => VariableType::Raw(e.size as usize),
            },
            Some(TypeDef::Struct(s)) | Some(TypeDef::Union(s)) => {
                VariableType::Raw(s.size as usize)
            }
            Some(TypeDef::Array { element, count }) => {
                let elem_size = self.type_size(*element).unwrap_or(0);
                let total = elem_size.checked_mul(count.unwrap_or(1)).unwrap_or(0);
                VariableType::Raw(total as usize)
            }
            Some(TypeDef::Subroutine { .. }) => VariableType::U32,
            Some(TypeDef::Unknown { size }) => VariableType::Raw(*size as usize),
            _ => VariableType::Raw(0),
        }
    }

    /// Iterate over all types
    pub fn iter(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.types
            .iter()
            .enumerate()
            .map(|(i, def)| (TypeId(i as u32), def))
    }

    /// Get statistics about the type table
    pub fn stats(&self) -> TypeTableStats {
        let mut stats = TypeTableStats {
            total_types: self.types.len(),
            ..Default::default()
        };

        for def in &self.types {
            match def {
                TypeDef::Primitive(_) => stats.primitives += 1,
                TypeDef::Pointer(_) => stats.pointers += 1,
                TypeDef::Reference(_) => stats.references += 1,
                TypeDef::Array { .. } => stats.arrays += 1,
                TypeDef::Struct(_) => stats.structs += 1,
                TypeDef::Union(_) => stats.unions += 1,
                TypeDef::Enum(_) => stats.enums += 1,
                TypeDef::Typedef { .. } => stats.typedefs += 1,
                TypeDef::Const(_) | TypeDef::Volatile(_) | TypeDef::Restrict(_) => {
                    stats.qualifiers += 1
                }
                TypeDef::Subroutine { .. } => stats.subroutines += 1,
                TypeDef::Void => stats.voids += 1,
                TypeDef::Unknown { .. } => stats.unknowns += 1,
                TypeDef::ForwardDecl { target, .. } => {
                    stats.forward_decls += 1;
                    if target.is_none() {
                        stats.unresolved_forward_decls += 1;
                    }
                }
                TypeDef::Placeholder => stats.placeholders += 1,
            }
        }

        stats
    }
}

/// Statistics about a type table
#[derive(Debug, Default)]
pub struct TypeTableStats {
    pub total_types: usize,
    pub primitives: usize,
    pub pointers: usize,
    pub references: usize,
    pub arrays: usize,
    pub structs: usize,
    pub unions: usize,
    pub enums: usize,
    pub typedefs: usize,
    pub qualifiers: usize,
    pub subroutines: usize,
    pub voids: usize,
    pub unknowns: usize,
    pub forward_decls: usize,
    pub unresolved_forward_decls: usize,
    pub placeholders: usize,
}

/// Shared type table wrapped in Arc for zero-cost cloning
pub type SharedTypeTable = Arc<TypeTable>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A member's `DW_FORM_ref_addr` to a `u16` declared later in the unit, as
    /// rustc emits with LTO: the definition reaches both keys.
    #[test]
    fn global_reference_before_its_type_is_filled() {
        let mut table = TypeTable::new();
        let global = GlobalTypeKey::global(0xdfe1).to_dwarf_key();
        let early = table.get_or_allocate(global);

        let local = DwarfTypeKey::new(0, 0xdfe1);
        let id = table.insert_for_key(local, TypeDef::Primitive(PrimitiveDef::UnsignedShort));
        table.mark_rust(id);
        table.set_source_name(id, "u16".into());
        table.register_alias(global, id);
        table.fill_placeholder(early, id);

        assert!(matches!(
            table.get(early),
            Some(TypeDef::Primitive(PrimitiveDef::UnsignedShort))
        ));
        assert_eq!(table.type_name(early), table.type_name(id));
        assert!(table.is_rust(early));
        assert_eq!(table.stats().placeholders, 0);
    }

    /// A typedef whose `DW_AT_type` names itself must not overflow the stack.
    #[test]
    fn self_referencing_typedef_is_bounded() {
        let mut table = TypeTable::new();
        let id = table.allocate();
        *table.get_mut(id).unwrap() = TypeDef::Typedef {
            name: "loop_t".into(),
            underlying: id,
        };

        assert_eq!(table.type_size(id), None);
        assert!(!table.is_addable(id));
        assert!(matches!(table.to_variable_type(id), VariableType::Raw(0)));
        assert_eq!(table.resolve(id), id);
    }

    #[test]
    fn self_referencing_forward_decl_resolves_in_bounded_steps() {
        let mut table = TypeTable::new();
        let id = table.allocate();
        *table.get_mut(id).unwrap() = TypeDef::ForwardDecl {
            name: "S".into(),
            kind: ForwardDeclKind::Struct,
            target: Some(id),
        };

        assert_eq!(table.resolve(id), id);
        assert_eq!(table.type_size(id), None);
    }

    #[test]
    fn self_referencing_array_is_bounded() {
        let mut table = TypeTable::new();
        let id = table.allocate();
        *table.get_mut(id).unwrap() = TypeDef::Array {
            element: id,
            count: Some(2),
        };

        assert_eq!(table.type_size(id), None);
        assert!(matches!(table.to_variable_type(id), VariableType::Raw(0)));
    }

    #[test]
    fn array_size_overflow_is_none() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let id = table.insert(TypeDef::Array {
            element: int_id,
            count: Some(u64::MAX),
        });

        assert_eq!(table.type_size(id), None);
    }

    #[test]
    fn test_type_table_basics() {
        let mut table = TypeTable::new();
        assert!(table.is_empty());

        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        assert_eq!(table.len(), 1);
        assert!(!table.is_empty());

        let def = table.get(int_id).unwrap();
        assert!(matches!(def, TypeDef::Primitive(PrimitiveDef::Int)));
        assert_eq!(table.type_size(int_id), Some(4));
        assert_eq!(table.type_name(int_id), "int");
    }

    #[test]
    fn test_dwarf_key_mapping() {
        let mut table = TypeTable::new();
        let key = DwarfTypeKey::new(0, 100);

        let id1 = table.get_or_allocate(key);
        let id2 = table.get_or_allocate(key);
        assert_eq!(id1, id2);

        table.define(id1, TypeDef::Primitive(PrimitiveDef::Float));
        assert_eq!(table.type_name(id1), "float");
    }

    #[test]
    fn test_pointer_type() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let ptr_id = table.insert(TypeDef::Pointer(int_id));

        assert_eq!(table.type_name(ptr_id), "int*");
        assert_eq!(table.type_size(ptr_id), Some(4));
    }

    #[test]
    fn test_struct_type() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        let mut struct_def = StructDef::new(Some("Point".to_string()), 8, false);
        struct_def
            .members
            .push(MemberDef::new("x".to_string(), 0, int_id));
        struct_def
            .members
            .push(MemberDef::new("y".to_string(), 4, int_id));

        let struct_id = table.insert(TypeDef::Struct(struct_def));

        assert_eq!(table.type_name(struct_id), "Point");
        assert!(table.is_expandable(struct_id));

        let members = table.members(struct_id).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].name, "x");
        assert_eq!(members[1].name, "y");
    }

    #[test]
    fn test_forward_declaration_resolution() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        let fwd_id = table.insert(TypeDef::ForwardDecl {
            name: "Point".to_string(),
            kind: ForwardDeclKind::Struct,
            target: None,
        });

        let mut struct_def = StructDef::new(Some("Point".to_string()), 8, false);
        struct_def
            .members
            .push(MemberDef::new("x".to_string(), 0, int_id));
        let def_id = table.insert(TypeDef::Struct(struct_def));

        assert!(table.get(fwd_id).unwrap().is_unresolved_forward_decl());

        table.resolve_forward_declarations();

        let resolved = table.resolve(fwd_id);
        assert_eq!(resolved, def_id);
        assert!(table.is_expandable(fwd_id));
    }

    #[test]
    fn test_typedef() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let typedef_id = table.insert(TypeDef::Typedef {
            name: "int32_t".to_string(),
            underlying: int_id,
        });

        assert_eq!(table.type_name(typedef_id), "int32_t");
        assert_eq!(table.underlying(typedef_id), int_id);
        assert_eq!(table.type_size(typedef_id), Some(4));
    }

    #[test]
    fn test_array_type() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let array_id = table.insert(TypeDef::Array {
            element: int_id,
            count: Some(10),
        });

        assert_eq!(table.type_name(array_id), "int[10]");
        assert_eq!(table.type_size(array_id), Some(40));
    }

    #[test]
    fn test_template_struct() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        let mut struct_def = StructDef::new(Some("vector".to_string()), 12, true);
        struct_def.template_params.push(TemplateParam::Type {
            name: "int".to_string(),
            type_id: int_id,
        });
        struct_def
            .members
            .push(MemberDef::new("data".to_string(), 0, int_id));

        let struct_id = table.insert(TypeDef::Struct(struct_def));

        assert_eq!(table.type_name(struct_id), "vector<int>");
    }

    #[test]
    fn test_stats() {
        let mut table = TypeTable::new();
        table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        table.insert(TypeDef::Primitive(PrimitiveDef::Float));
        table.insert(TypeDef::Void);
        table.insert(TypeDef::ForwardDecl {
            name: "Foo".to_string(),
            kind: ForwardDeclKind::Struct,
            target: None,
        });

        let stats = table.stats();
        assert_eq!(stats.total_types, 4);
        assert_eq!(stats.primitives, 2);
        assert_eq!(stats.voids, 1);
        assert_eq!(stats.forward_decls, 1);
        assert_eq!(stats.unresolved_forward_decls, 1);
    }

    #[test]
    fn test_typedef_chain_resolution() {
        let mut table = TypeTable::new();

        // Create a chain: typedef -> typedef -> primitive
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let int32_id = table.insert(TypeDef::Typedef {
            name: "int32_t".to_string(),
            underlying: int_id,
        });
        let myint_id = table.insert(TypeDef::Typedef {
            name: "MyInt".to_string(),
            underlying: int32_id,
        });

        // Should resolve through the chain
        let final_type = table.underlying(myint_id);
        assert_eq!(final_type, int_id);
        assert_eq!(table.type_size(myint_id), Some(4));
    }

    #[test]
    fn test_recursive_struct_detection() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        // Create a struct with a pointer to itself (linked list node)
        let node_fwd = table.insert(TypeDef::ForwardDecl {
            name: "Node".to_string(),
            kind: ForwardDeclKind::Struct,
            target: None,
        });

        let node_ptr_id = table.insert(TypeDef::Pointer(node_fwd));

        let mut node_def = StructDef::new(Some("Node".to_string()), 12, false);
        node_def
            .members
            .push(MemberDef::new("data".to_string(), 0, int_id));
        node_def
            .members
            .push(MemberDef::new("next".to_string(), 4, node_ptr_id));
        let node_id = table.insert(TypeDef::Struct(node_def));

        // Resolve forward declarations
        table.resolve_forward_declarations();

        // Should be able to navigate the structure without infinite loops
        assert_eq!(table.type_name(node_id), "Node");
        assert!(table.is_expandable(node_id));

        let members = table.members(node_id).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[1].name, "next");
    }

    #[test]
    fn test_template_parameter_handling() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let float_id = table.insert(TypeDef::Primitive(PrimitiveDef::Float));

        // Create a templated struct with both type and value parameters
        let mut struct_def = StructDef::new(Some("Container".to_string()), 16, true);
        struct_def.template_params.push(TemplateParam::Type {
            name: "int".to_string(),
            type_id: int_id,
        });
        struct_def.template_params.push(TemplateParam::Value {
            name: "10".to_string(),
            value: 10,
        });
        struct_def
            .members
            .push(MemberDef::new("data".to_string(), 0, float_id));

        let struct_id = table.insert(TypeDef::Struct(struct_def));

        // Check name includes all template parameters
        let name = table.type_name(struct_id);
        assert!(name.contains("Container"));
        assert!(name.contains("int"));
        assert!(name.contains("10"));
    }

    #[test]
    fn test_enum_type() {
        let mut table = TypeTable::new();

        let mut enum_def = EnumDef::new(Some("Color".to_string()), 4, false);
        enum_def.variants.push(EnumVariant {
            name: "Red".to_string(),
            value: 0,
        });
        enum_def.variants.push(EnumVariant {
            name: "Green".to_string(),
            value: 1,
        });
        enum_def.variants.push(EnumVariant {
            name: "Blue".to_string(),
            value: 2,
        });

        let enum_id = table.insert(TypeDef::Enum(enum_def));

        assert_eq!(table.type_name(enum_id), "Color");
        assert_eq!(table.type_size(enum_id), Some(4));
        // Enums may not be marked as expandable in the same way as structs
        // Just check the type exists
        assert!(table.get(enum_id).is_some());
    }

    #[test]
    fn test_union_type() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let float_id = table.insert(TypeDef::Primitive(PrimitiveDef::Float));

        let mut union_def = StructDef::new(Some("Value".to_string()), 4, false);
        union_def
            .members
            .push(MemberDef::new("as_int".to_string(), 0, int_id));
        union_def
            .members
            .push(MemberDef::new("as_float".to_string(), 0, float_id));

        let union_id = table.insert(TypeDef::Union(union_def));

        // Union name includes "union" prefix
        assert!(table.type_name(union_id).contains("Value"));
        assert_eq!(table.type_size(union_id), Some(4));
        assert!(table.is_expandable(union_id));

        let members = table.members(union_id).unwrap();
        assert_eq!(members.len(), 2);
        // Both members should be at offset 0
        assert_eq!(members[0].offset, 0);
        assert_eq!(members[1].offset, 0);
    }

    #[test]
    fn test_void_pointer() {
        let mut table = TypeTable::new();
        let void_id = table.insert(TypeDef::Void);
        let void_ptr_id = table.insert(TypeDef::Pointer(void_id));

        assert_eq!(table.type_name(void_ptr_id), "void*");
        assert_eq!(table.type_size(void_ptr_id), Some(4));
    }

    #[test]
    fn test_double_pointer() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let ptr1_id = table.insert(TypeDef::Pointer(int_id));
        let ptr2_id = table.insert(TypeDef::Pointer(ptr1_id));

        assert_eq!(table.type_name(ptr2_id), "int**");
        assert_eq!(table.type_size(ptr2_id), Some(4));
    }

    #[test]
    fn test_array_of_structs() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        let mut struct_def = StructDef::new(Some("Point".to_string()), 8, false);
        struct_def
            .members
            .push(MemberDef::new("x".to_string(), 0, int_id));
        struct_def
            .members
            .push(MemberDef::new("y".to_string(), 4, int_id));
        let struct_id = table.insert(TypeDef::Struct(struct_def));

        let array_id = table.insert(TypeDef::Array {
            element: struct_id,
            count: Some(5),
        });

        assert_eq!(table.type_name(array_id), "Point[5]");
        assert_eq!(table.type_size(array_id), Some(40)); // 5 * 8 bytes
    }

    #[test]
    fn test_variable_length_array() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        let array_id = table.insert(TypeDef::Array {
            element: int_id,
            count: None, // Unknown size
        });

        assert_eq!(table.type_name(array_id), "int[]");
        // Arrays without count have size 0, not None
        let size = table.type_size(array_id);
        assert!(size == Some(0) || size.is_none());
    }

    #[test]
    fn test_reference_type() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let ref_id = table.insert(TypeDef::Reference(int_id));

        assert_eq!(table.type_name(ref_id), "int&");
        assert_eq!(table.type_size(ref_id), Some(4));
    }

    #[test]
    fn test_nested_structs() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        // Inner struct
        let mut inner = StructDef::new(Some("Inner".to_string()), 8, false);
        inner
            .members
            .push(MemberDef::new("a".to_string(), 0, int_id));
        inner
            .members
            .push(MemberDef::new("b".to_string(), 4, int_id));
        let inner_id = table.insert(TypeDef::Struct(inner));

        // Outer struct containing inner struct
        let mut outer = StructDef::new(Some("Outer".to_string()), 12, false);
        outer
            .members
            .push(MemberDef::new("inner".to_string(), 0, inner_id));
        outer
            .members
            .push(MemberDef::new("x".to_string(), 8, int_id));
        let outer_id = table.insert(TypeDef::Struct(outer));

        assert_eq!(table.type_size(outer_id), Some(12));

        let members = table.members(outer_id).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].name, "inner");
        assert_eq!(members[0].type_id, inner_id);
    }

    #[test]
    fn test_const_volatile_qualifiers() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let const_id = table.insert(TypeDef::Const(int_id));
        let volatile_id = table.insert(TypeDef::Volatile(int_id));

        // Qualifiers should be transparent for size
        assert_eq!(table.type_size(const_id), Some(4));
        assert_eq!(table.type_size(volatile_id), Some(4));

        // Names should reflect qualifiers
        assert!(table.type_name(const_id).contains("const"));
        assert!(table.type_name(volatile_id).contains("volatile"));
    }

    #[test]
    fn test_shared_type_table() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));

        let shared = SharedTypeTable::new(table);

        // Should be able to clone and share
        let shared2 = shared.clone();

        assert_eq!(shared.type_name(int_id), "int");
        assert_eq!(shared2.type_name(int_id), "int");
    }
}
