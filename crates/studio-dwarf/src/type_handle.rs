//! A handle that pairs a shared [`TypeTable`] with a [`TypeId`] for direct type access.

use crate::type_table::TypeTable;
use crate::types::*;
use crate::variable_type::VariableType;
use std::sync::Arc;

/// A handle to a type that wraps `Arc<TypeTable>` + TypeId.
/// This provides zero-overhead cloning and direct method access.
#[derive(Debug, Clone)]
pub struct TypeHandle {
    table: Arc<TypeTable>,
    id: TypeId,
}

impl TypeHandle {
    /// Create a new TypeHandle
    pub fn new(table: Arc<TypeTable>, id: TypeId) -> Self {
        Self { table, id }
    }

    /// Create an invalid TypeHandle
    pub fn invalid(table: Arc<TypeTable>) -> Self {
        Self {
            table,
            id: TypeId::INVALID,
        }
    }

    /// Get the TypeId
    pub fn id(&self) -> TypeId {
        self.id
    }

    /// Check if this is a valid type
    pub fn is_valid(&self) -> bool {
        self.id.is_valid() && self.table.get(self.id).is_some()
    }

    /// Get the type definition
    pub fn def(&self) -> Option<&TypeDef> {
        self.table.get(self.id)
    }

    /// Get the resolved type definition (follows forward declarations)
    pub fn resolved_def(&self) -> Option<&TypeDef> {
        self.table.get_resolved(self.id)
    }

    /// Get the resolved TypeHandle
    pub fn resolved(&self) -> TypeHandle {
        TypeHandle::new(self.table.clone(), self.table.resolve(self.id))
    }

    /// Get the type name
    pub fn type_name(&self) -> String {
        self.table.type_name(self.id)
    }

    /// Get the type size in bytes
    pub fn size(&self) -> Option<u64> {
        self.table.type_size(self.id)
    }

    /// Check if this type is expandable
    pub fn is_expandable(&self) -> bool {
        self.table.is_expandable(self.id)
    }

    /// Check if this type can be added as a variable
    pub fn is_addable(&self) -> bool {
        self.table.is_addable(self.id)
    }

    /// Get the members of this type (if struct/union)
    pub fn members(&self) -> Option<&[MemberDef]> {
        self.table.get_members(self.id)
    }

    /// Get the underlying type (strips typedefs, qualifiers)
    pub fn underlying(&self) -> TypeHandle {
        TypeHandle::new(self.table.clone(), self.table.get_underlying(self.id))
    }

    /// Convert to VariableType
    pub fn to_variable_type(&self) -> VariableType {
        self.table.to_variable_type(self.id)
    }

    /// Get a handle to a member's type
    pub fn member_type(&self, member: &MemberDef) -> TypeHandle {
        TypeHandle::new(self.table.clone(), member.type_id)
    }

    /// Get a reference to the underlying type table
    pub fn table(&self) -> &Arc<TypeTable> {
        &self.table
    }

    /// Definition after stripping typedefs, qualifiers and forward declarations.
    /// Kind checks use this so `volatile Foo` and `typedef Foo *Bar` classify
    /// the same as `Foo` and `Foo *`.
    fn underlying_def(&self) -> Option<&TypeDef> {
        self.table.get(self.table.get_underlying(self.id))
    }

    /// Check if this is a struct or union
    pub fn is_struct_or_union(&self) -> bool {
        matches!(
            self.underlying_def(),
            Some(TypeDef::Struct(_)) | Some(TypeDef::Union(_))
        )
    }

    /// Check if this is a pointer or reference
    pub fn is_pointer_or_reference(&self) -> bool {
        matches!(
            self.underlying_def(),
            Some(TypeDef::Pointer(_)) | Some(TypeDef::Reference(_))
        )
    }

    /// Check if this is a primitive type
    pub fn is_primitive(&self) -> bool {
        matches!(self.underlying_def(), Some(TypeDef::Primitive(_)))
    }

    /// Get the pointee type if this is a pointer/reference
    pub fn pointee(&self) -> Option<TypeHandle> {
        match self.underlying_def() {
            Some(TypeDef::Pointer(inner)) | Some(TypeDef::Reference(inner)) => {
                Some(TypeHandle::new(self.table.clone(), *inner))
            }
            _ => None,
        }
    }

    /// Get the array element type if this is an array
    pub fn element_type(&self) -> Option<TypeHandle> {
        match self.underlying_def() {
            Some(TypeDef::Array { element, .. }) => {
                Some(TypeHandle::new(self.table.clone(), *element))
            }
            _ => None,
        }
    }

    /// Get the array count if this is an array
    pub fn array_count(&self) -> Option<u64> {
        match self.underlying_def() {
            Some(TypeDef::Array { count, .. }) => *count,
            _ => None,
        }
    }

    /// Check if this type is an array
    pub fn is_array(&self) -> bool {
        matches!(self.underlying_def(), Some(TypeDef::Array { .. }))
    }

    /// Get the element size if this is an array
    pub fn element_size(&self) -> Option<u64> {
        match self.underlying_def() {
            Some(TypeDef::Array { element, .. }) => self.table.type_size(*element),
            _ => None,
        }
    }

    /// Check if this is a pointer type that can be expanded to show its pointee type
    /// Returns true if this is a pointer/reference to a struct, union, or array
    pub fn is_pointer_expandable(&self) -> bool {
        if let Some(pointee) = self.pointee() {
            let pointee_underlying = pointee.underlying();
            pointee_underlying.is_struct_or_union() || pointee_underlying.is_array()
        } else {
            false
        }
    }

    /// Get the pointee's underlying type handle for pointer expansion
    /// This allows accessing the struct/union members that the pointer points to
    pub fn pointee_underlying(&self) -> Option<TypeHandle> {
        self.pointee().map(|p| p.underlying())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_handle() {
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

        let shared = Arc::new(table);
        let handle = TypeHandle::new(shared.clone(), struct_id);

        assert!(handle.is_valid());
        assert_eq!(handle.type_name(), "Point");
        assert!(handle.is_expandable());
        assert!(handle.is_struct_or_union());

        let members = handle.members().unwrap();
        assert_eq!(members.len(), 2);

        let x_type = handle.member_type(&members[0]);
        assert!(x_type.is_primitive());
        assert_eq!(x_type.type_name(), "int");
    }

    #[test]
    fn test_type_handle_pointer() {
        let mut table = TypeTable::new();
        let int_id = table.insert(TypeDef::Primitive(PrimitiveDef::Int));
        let ptr_id = table.insert(TypeDef::Pointer(int_id));

        let shared = Arc::new(table);
        let handle = TypeHandle::new(shared, ptr_id);

        assert!(handle.is_pointer_or_reference());
        let pointee = handle.pointee().unwrap();
        assert!(pointee.is_primitive());
        assert_eq!(pointee.type_name(), "int");
    }
}
