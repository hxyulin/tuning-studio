//! Type definitions stored in the type table: [`TypeId`], [`TypeDef`] and the struct,
//! enum and primitive records it refers to.

use crate::variable_type::VariableType;

/// A unique identifier for a type in the global type table.
/// This is just an index into the types vector.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub struct TypeId(pub u32);

impl TypeId {
    /// The invalid/unresolved type ID
    pub const INVALID: TypeId = TypeId(u32::MAX);

    /// Check if this is a valid type ID
    pub fn is_valid(self) -> bool {
        self != Self::INVALID
    }
}

impl std::fmt::Display for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if *self == Self::INVALID {
            write!(f, "TypeId(INVALID)")
        } else {
            write!(f, "TypeId({})", self.0)
        }
    }
}

/// Key for mapping DWARF DIE locations to TypeIds
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DwarfTypeKey {
    pub unit_index: usize,
    pub offset: usize,
}

impl DwarfTypeKey {
    pub fn new(unit_index: usize, offset: usize) -> Self {
        Self { unit_index, offset }
    }
}

/// A unified key that can represent either a unit-local or global DWARF type reference.
/// ARM compilers often use DW_FORM_ref_addr (global offsets) instead of DW_FORM_ref* (unit-local).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlobalTypeKey {
    /// Unit-local reference (unit_index + local offset within the unit)
    UnitLocal(DwarfTypeKey),
    /// Global reference (absolute offset in .debug_info section)
    /// We use usize::MAX as a sentinel unit_index for global refs
    Global(usize),
}

impl GlobalTypeKey {
    /// Create a unit-local key
    pub fn unit_local(unit_index: usize, offset: usize) -> Self {
        GlobalTypeKey::UnitLocal(DwarfTypeKey::new(unit_index, offset))
    }

    /// Create a global key from an absolute .debug_info offset
    pub fn global(offset: usize) -> Self {
        GlobalTypeKey::Global(offset)
    }

    /// Convert to DwarfTypeKey for backwards compatibility
    /// Global keys use a sentinel unit_index of usize::MAX
    pub fn to_dwarf_key(&self) -> DwarfTypeKey {
        match self {
            GlobalTypeKey::UnitLocal(key) => *key,
            GlobalTypeKey::Global(offset) => DwarfTypeKey::new(usize::MAX, *offset),
        }
    }
}

impl From<DwarfTypeKey> for GlobalTypeKey {
    fn from(key: DwarfTypeKey) -> Self {
        GlobalTypeKey::UnitLocal(key)
    }
}

/// A type definition in the type table
#[derive(Debug, Clone)]
pub enum TypeDef {
    /// Primitive/base types (int, float, bool, etc.)
    Primitive(PrimitiveDef),
    /// Pointer to another type
    Pointer(TypeId),
    /// Reference to another type (C++)
    Reference(TypeId),
    /// Array of elements
    Array { element: TypeId, count: Option<u64> },
    /// Struct or class
    Struct(StructDef),
    /// Union type
    Union(StructDef),
    /// Enumeration
    Enum(EnumDef),
    /// Typedef/alias
    Typedef { name: String, underlying: TypeId },
    /// Const qualifier
    Const(TypeId),
    /// Volatile qualifier
    Volatile(TypeId),
    /// Restrict qualifier
    Restrict(TypeId),
    /// Subroutine/function type
    Subroutine {
        return_type: Option<TypeId>,
        params: Vec<TypeId>,
    },
    /// Void type
    Void,
    /// Unknown type with size
    Unknown { size: u64 },
    /// Forward declaration - will be resolved to point to actual definition
    ForwardDecl {
        name: String,
        kind: ForwardDeclKind,
        /// Once resolved, this points to the actual type
        target: Option<TypeId>,
    },
    /// Placeholder for types being parsed (used during construction)
    Placeholder,
}

impl TypeDef {
    /// Check if this is a forward declaration that hasn't been resolved
    pub fn is_unresolved_forward_decl(&self) -> bool {
        matches!(self, TypeDef::ForwardDecl { target: None, .. })
    }

    /// Check if this is a forward declaration (resolved or not)
    pub fn is_forward_decl(&self) -> bool {
        matches!(self, TypeDef::ForwardDecl { .. })
    }

    /// Get the name of this type if it has one
    pub fn name(&self) -> Option<&str> {
        match self {
            TypeDef::Struct(s) | TypeDef::Union(s) => s.name.as_deref(),
            TypeDef::Enum(e) => e.name.as_deref(),
            TypeDef::Typedef { name, .. } => Some(name),
            TypeDef::ForwardDecl { name, .. } => Some(name),
            TypeDef::Primitive(p) => Some(p.name()),
            _ => None,
        }
    }

    /// Check if this type can be expanded (has members)
    pub fn is_expandable(&self) -> bool {
        match self {
            TypeDef::Struct(s) | TypeDef::Union(s) => !s.members.is_empty(),
            TypeDef::ForwardDecl {
                target: Some(_), ..
            } => true, // Will check target
            _ => false,
        }
    }
}

/// The kind of forward declaration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardDeclKind {
    Struct,
    Class,
    Union,
    Enum,
}

/// Primitive type definition
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveDef {
    Bool,
    Char,
    /// Unicode code unit; unlike C char, its DWARF byte size is significant.
    UnicodeChar {
        size: u64,
    },
    SignedChar,
    UnsignedChar,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
    Float,
    Double,
    LongDouble,
    /// Sized integer (for unusual sizes)
    SizedInt {
        size: u64,
        signed: bool,
    },
    /// Sized float (for unusual sizes)
    SizedFloat {
        size: u64,
    },
}

impl PrimitiveDef {
    /// Get the size in bytes
    pub fn size(&self) -> u64 {
        match self {
            PrimitiveDef::Bool
            | PrimitiveDef::Char
            | PrimitiveDef::SignedChar
            | PrimitiveDef::UnsignedChar => 1,
            PrimitiveDef::Short | PrimitiveDef::UnsignedShort => 2,
            PrimitiveDef::Int | PrimitiveDef::UnsignedInt | PrimitiveDef::Float => 4,
            PrimitiveDef::Long | PrimitiveDef::UnsignedLong => 4,
            PrimitiveDef::LongLong | PrimitiveDef::UnsignedLongLong | PrimitiveDef::Double => 8,
            PrimitiveDef::LongDouble => 16,
            PrimitiveDef::SizedInt { size, .. } => *size,
            PrimitiveDef::SizedFloat { size } | PrimitiveDef::UnicodeChar { size } => *size,
        }
    }

    /// Get the name of this primitive type
    pub fn name(&self) -> &'static str {
        match self {
            PrimitiveDef::Bool => "bool",
            PrimitiveDef::Char | PrimitiveDef::UnicodeChar { .. } => "char",
            PrimitiveDef::SignedChar => "signed char",
            PrimitiveDef::UnsignedChar => "unsigned char",
            PrimitiveDef::Short => "short",
            PrimitiveDef::UnsignedShort => "unsigned short",
            PrimitiveDef::Int => "int",
            PrimitiveDef::UnsignedInt => "unsigned int",
            PrimitiveDef::Long => "long",
            PrimitiveDef::UnsignedLong => "unsigned long",
            PrimitiveDef::LongLong => "long long",
            PrimitiveDef::UnsignedLongLong => "unsigned long long",
            PrimitiveDef::Float => "float",
            PrimitiveDef::Double => "double",
            PrimitiveDef::LongDouble => "long double",
            PrimitiveDef::SizedInt { signed: true, .. } => "int",
            PrimitiveDef::SizedInt { signed: false, .. } => "unsigned int",
            PrimitiveDef::SizedFloat { .. } => "float",
        }
    }

    /// Check if this is a signed type
    pub fn is_signed(&self) -> bool {
        match self {
            PrimitiveDef::SignedChar
            | PrimitiveDef::Short
            | PrimitiveDef::Int
            | PrimitiveDef::Long
            | PrimitiveDef::LongLong => true,
            PrimitiveDef::SizedInt { signed, .. } => *signed,
            PrimitiveDef::Float
            | PrimitiveDef::Double
            | PrimitiveDef::LongDouble
            | PrimitiveDef::SizedFloat { .. } => true,
            _ => false,
        }
    }

    /// Check if this is a floating point type
    pub fn is_float(&self) -> bool {
        matches!(
            self,
            PrimitiveDef::Float
                | PrimitiveDef::Double
                | PrimitiveDef::LongDouble
                | PrimitiveDef::SizedFloat { .. }
        )
    }

    /// Convert to VariableType
    pub fn to_variable_type(&self) -> VariableType {
        match self {
            PrimitiveDef::UnicodeChar { size } => match size {
                1 => VariableType::U8,
                2 => VariableType::U16,
                4 => VariableType::U32,
                _ => VariableType::Raw(*size as usize),
            },
            PrimitiveDef::Bool => VariableType::Bool,
            PrimitiveDef::Char | PrimitiveDef::SignedChar => VariableType::I8,
            PrimitiveDef::UnsignedChar => VariableType::U8,
            PrimitiveDef::Short => VariableType::I16,
            PrimitiveDef::UnsignedShort => VariableType::U16,
            PrimitiveDef::Int | PrimitiveDef::Long => VariableType::I32,
            PrimitiveDef::UnsignedInt | PrimitiveDef::UnsignedLong => VariableType::U32,
            PrimitiveDef::LongLong => VariableType::I64,
            PrimitiveDef::UnsignedLongLong => VariableType::U64,
            PrimitiveDef::Float => VariableType::F32,
            PrimitiveDef::Double | PrimitiveDef::LongDouble => VariableType::F64,
            PrimitiveDef::SizedInt { size, signed } => match (size, signed) {
                (1, true) => VariableType::I8,
                (1, false) => VariableType::U8,
                (2, true) => VariableType::I16,
                (2, false) => VariableType::U16,
                (4, true) => VariableType::I32,
                (4, false) => VariableType::U32,
                (8, true) => VariableType::I64,
                (8, false) => VariableType::U64,
                _ => VariableType::Raw(*size as usize),
            },
            PrimitiveDef::SizedFloat { size } => match size {
                4 => VariableType::F32,
                8 => VariableType::F64,
                _ => VariableType::Raw(*size as usize),
            },
        }
    }
}

/// Struct/class/union definition
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: Option<String>,
    pub size: u64,
    pub members: Vec<MemberDef>,
    pub is_class: bool,
    /// Template parameters (empty for non-template types)
    pub template_params: Vec<TemplateParam>,
    /// Base classes for C++ inheritance
    pub base_classes: Vec<BaseClassDef>,
    /// Rust enum layout (`DW_TAG_variant_part`); `members` is empty when set
    pub variant_part: Option<VariantPart>,
}

impl StructDef {
    pub fn new(name: Option<String>, size: u64, is_class: bool) -> Self {
        Self {
            name,
            size,
            members: Vec::new(),
            is_class,
            template_params: Vec::new(),
            base_classes: Vec::new(),
            variant_part: None,
        }
    }

    /// Get the name as the compiler wrote it
    pub fn display_name_without_params(&self) -> String {
        self.name.as_deref().unwrap_or("<anonymous>").to_string()
    }

    /// Get the display name including template parameters
    pub fn display_name(&self) -> String {
        let base_name = self.name.as_deref().unwrap_or("<anonymous>");
        // rustc and GCC already spell the arguments into the name (`Atomic<u32>`)
        if self.template_params.is_empty() || base_name.contains('<') {
            base_name.to_string()
        } else {
            let params: Vec<String> = self.template_params.iter().map(|p| p.display()).collect();
            format!("{}<{}>", base_name, params.join(", "))
        }
    }
}

/// A member of a struct/union
#[derive(Debug, Clone)]
pub struct MemberDef {
    pub name: String,
    pub offset: u64,
    pub type_id: TypeId,
    pub bit_offset: Option<u64>,
    pub bit_size: Option<u64>,
}

impl MemberDef {
    pub fn new(name: String, offset: u64, type_id: TypeId) -> Self {
        Self {
            name,
            offset,
            type_id,
            bit_offset: None,
            bit_size: None,
        }
    }

    /// Check if this is a bit field
    pub fn is_bitfield(&self) -> bool {
        self.bit_size.is_some()
    }
}

/// Layout of a Rust enum with data, from `DW_TAG_variant_part`.
///
/// Read the `discriminant` member's raw bits and pass them to [`VariantPart::select`]
/// to find the active variant. For niche-encoded enums the discriminant lives inside
/// another variant's payload and the untagged (dataful) variant has no `discr_value`.
#[derive(Debug, Clone, Default)]
pub struct VariantPart {
    /// The tag member (offset/type within the enum). `None` for single-variant enums.
    pub discriminant: Option<MemberDef>,
    pub variants: Vec<VariantDef>,
}

impl VariantPart {
    /// Active variant for a raw discriminant value: exact match, else the default variant.
    pub fn select(&self, discr: u64) -> Option<&VariantDef> {
        self.variants
            .iter()
            .find(|v| v.discr_value == Some(discr))
            .or_else(|| self.variants.iter().find(|v| v.discr_value.is_none()))
    }
}

/// One enum variant; `member` is named after the variant and typed as its payload struct.
#[derive(Debug, Clone)]
pub struct VariantDef {
    pub discr_value: Option<u64>,
    pub member: MemberDef,
    /// Where the variant is declared. For an `async fn`'s state, a suspend
    /// variant points at its `.await`.
    pub location: Option<SourceLocation>,
}

/// A file and line from `DW_AT_decl_file` / `DW_AT_decl_line`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourceLocation {
    /// As the compiler recorded it; absolute for Rust builds
    pub file: String,
    pub line: u64,
}

/// Base class for C++ inheritance
#[derive(Debug, Clone)]
pub struct BaseClassDef {
    pub type_id: TypeId,
    pub offset: u64,
    pub is_virtual: bool,
}

/// Template parameter
#[derive(Debug, Clone)]
pub enum TemplateParam {
    /// Type parameter (e.g., T in `vector<T>`)
    Type { name: String, type_id: TypeId },
    /// Value parameter (e.g., N in `array<T, N>`)
    Value { name: String, value: i64 },
}

impl TemplateParam {
    pub fn display(&self) -> String {
        match self {
            TemplateParam::Type { name, .. } => name.clone(),
            TemplateParam::Value { value, .. } => value.to_string(),
        }
    }
}

/// Enumeration definition
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: Option<String>,
    pub size: u64,
    pub variants: Vec<EnumVariant>,
    pub is_scoped: bool,
    pub underlying_type: Option<TypeId>,
}

impl EnumDef {
    pub fn new(name: Option<String>, size: u64, is_scoped: bool) -> Self {
        Self {
            name,
            size,
            variants: Vec::new(),
            is_scoped,
            underlying_type: None,
        }
    }

    /// Find a variant by value
    pub fn find_variant(&self, value: i64) -> Option<&EnumVariant> {
        self.variants.iter().find(|v| v.value == value)
    }

    /// Convert a value to its variant name
    pub fn value_to_string(&self, value: i64) -> String {
        if let Some(variant) = self.find_variant(value) {
            if self.is_scoped
                && let Some(name) = &self.name
            {
                return format!("{}::{}", name, variant.name);
            }
            variant.name.clone()
        } else {
            value.to_string()
        }
    }
}

/// An enum variant/enumerator
#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    pub value: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_id() {
        let id = TypeId(42);
        assert!(id.is_valid());
        assert!(!TypeId::INVALID.is_valid());
        assert_eq!(format!("{}", id), "TypeId(42)");
        assert_eq!(format!("{}", TypeId::INVALID), "TypeId(INVALID)");
    }

    #[test]
    fn test_primitive_def() {
        assert_eq!(PrimitiveDef::Int.size(), 4);
        assert_eq!(PrimitiveDef::LongLong.size(), 8);
        assert!(PrimitiveDef::Int.is_signed());
        assert!(!PrimitiveDef::UnsignedInt.is_signed());
        assert!(PrimitiveDef::Float.is_float());
        assert!(!PrimitiveDef::Int.is_float());
    }
}
