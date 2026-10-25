/// Numeric storage types used by the migrated DWARF type table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    U8,
    U16,
    #[default]
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
    Raw(usize),
}

impl std::fmt::Display for VariableType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::U8 => write!(formatter, "u8"),
            Self::U16 => write!(formatter, "u16"),
            Self::U32 => write!(formatter, "u32"),
            Self::U64 => write!(formatter, "u64"),
            Self::I8 => write!(formatter, "i8"),
            Self::I16 => write!(formatter, "i16"),
            Self::I32 => write!(formatter, "i32"),
            Self::I64 => write!(formatter, "i64"),
            Self::F32 => write!(formatter, "f32"),
            Self::F64 => write!(formatter, "f64"),
            Self::Bool => write!(formatter, "bool"),
            Self::Raw(size) => write!(formatter, "{size} bytes"),
        }
    }
}
