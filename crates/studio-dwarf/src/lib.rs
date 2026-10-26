//! ELF symbol and DWARF type inspection for Tuning Studio.
//!
//! Ported from `datavis-rs` (MIT, `src/backend/{elf_parser,dwarf_parser,type_table}.rs`
//! at `cb4c641`). The parser and type table are kept as they were; the ELF layer
//! drops the datavis `Variable` coupling and gains [`diff_symbols`] for re-resolving
//! symbols after a firmware rebuild and [`tree`] for browsing statics by path.

mod dwarf_parser;
mod elf;
mod error;
mod rebuild;
mod type_handle;
mod type_table;
mod types;
mod variable_type;

pub mod task_stats;
pub mod task_trace;
pub mod tasks;
pub mod tree;

#[cfg(test)]
mod fixture_tests;

pub use dwarf_parser::{DwarfDiagnostics, VariableStatus};
pub use elf::{ElfInfo, ElfParser, SymbolInfo, SymbolType, demangle_symbol};
pub use error::{Error, Result};
pub use rebuild::{SymbolChange, TrackedSymbol, diff_symbols};
pub use type_handle::TypeHandle;
pub use type_table::{SharedTypeTable, TypeTable, TypeTableStats};
pub use types::{
    BaseClassDef, EnumDef, EnumVariant, ForwardDeclKind, MemberDef, PrimitiveDef, SourceLocation,
    StructDef, TemplateParam, TypeDef, TypeId, VariantDef, VariantPart,
};
pub use variable_type::VariableType;
