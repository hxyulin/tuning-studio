//! ELF/AXF Symbol Parser with DWARF Debug Info and C++ Demangling
//!
//! This module provides functionality to parse ELF and AXF (ARM Executable Format)
//! files to extract variable symbols with their addresses, types, and structure information.
//! It supports:
//! - Symbol extraction from symbol tables
//! - DWARF debug info parsing for type information
//! - C++ and Rust symbol demangling
//! - Struct/class member parsing
//! - Enum parsing
//! - Full type reference resolution (pointers, arrays, typedefs, etc.)

use crate::dwarf_parser::{DwarfDiagnostics, DwarfParser, ParsedSymbol, VariableStatus};
use crate::error::{Error, Result};
use crate::type_table::{MemberDef, SharedTypeTable, TypeHandle, TypeId, TypeTable};
use crate::variable_type::VariableType;
use cpp_demangle::Symbol as CppSymbol;
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Information about a symbol extracted from an ELF file
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    /// Original (mangled) symbol name
    pub mangled_name: String,
    /// Demangled symbol name (if applicable)
    pub demangled_name: String,
    /// Short display name (without full namespace for C++)
    pub display_name: String,
    /// Memory address
    pub address: u64,
    /// Size in bytes
    pub size: u64,
    /// Symbol type
    pub symbol_type: SymbolType,
    /// Section name where the symbol resides
    pub section: String,
    /// Whether this is a global symbol
    pub is_global: bool,
    /// Address lies in an allocated, writable section (RAM that changes at runtime)
    pub writable: bool,
    /// Type ID referencing the type table (if available)
    pub type_id: Option<TypeId>,
    /// Status indicating why this variable can or cannot be read (from DWARF)
    pub status: Option<VariableStatus>,
}

impl SymbolInfo {
    /// Infer variable type from symbol size (type table version available via ElfInfo)
    pub fn infer_variable_type(&self) -> VariableType {
        // Fall back to size-based inference
        // For type-aware inference, use ElfInfo::infer_variable_type_for_symbol
        match self.size {
            1 => VariableType::U8,
            2 => VariableType::U16,
            4 => VariableType::U32,
            8 => VariableType::U64,
            _ => VariableType::Raw(self.size as usize),
        }
    }

    /// Get the best name to display
    pub fn name(&self) -> &str {
        &self.display_name
    }

    /// Check if this symbol matches a search query
    pub fn matches(&self, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        self.display_name.to_lowercase().contains(&query_lower)
            || self.demangled_name.to_lowercase().contains(&query_lower)
            || self.mangled_name.to_lowercase().contains(&query_lower)
    }

    /// Check if this symbol is a complex type (requires type table)
    /// Use ElfInfo::is_symbol_complex for type-aware check
    pub fn is_complex(&self) -> bool {
        // Without type table access, use size heuristic
        self.size > 8
    }

    /// Get a TypeHandle for this symbol's type (requires shared type table)
    pub fn type_handle(&self, table: &SharedTypeTable) -> Option<TypeHandle> {
        self.type_id.map(|id| TypeHandle::new(table.clone(), id))
    }

    /// Check if this symbol can be read (has a valid address)
    pub fn is_readable(&self) -> bool {
        self.status.as_ref().is_none_or(|s| s.is_readable())
    }

    /// Get the reason why this symbol cannot be read, if applicable
    pub fn unreadable_reason(&self) -> Option<&'static str> {
        match &self.status {
            Some(status) if !status.is_readable() => Some(status.reason()),
            _ => None,
        }
    }

    /// Get detailed reason for unreadability
    pub fn detailed_unreadable_reason(&self) -> Option<String> {
        match &self.status {
            Some(status) if !status.is_readable() => Some(status.detailed_reason()),
            _ => None,
        }
    }
}

/// Type of symbol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolType {
    /// Data object (variable)
    Variable,
    /// Function
    Function,
    /// Section
    Section,
    /// File
    File,
    /// Unknown or other
    Other,
}

impl std::fmt::Display for SymbolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolType::Variable => write!(f, "Variable"),
            SymbolType::Function => write!(f, "Function"),
            SymbolType::Section => write!(f, "Section"),
            SymbolType::File => write!(f, "File"),
            SymbolType::Other => write!(f, "Other"),
        }
    }
}

/// Parsed ELF file information
#[derive(Debug, Clone)]
pub struct ElfInfo {
    /// Path to the ELF file
    pub path: String,
    /// Entry point address
    pub entry_point: u64,
    /// Whether this is a 64-bit ELF
    pub is_64bit: bool,
    /// Whether this is little-endian
    pub is_little_endian: bool,
    /// Machine type (e.g., ARM, x86)
    pub machine: String,
    /// All symbols found
    pub symbols: Vec<SymbolInfo>,
    /// Symbols indexed by name
    symbols_by_name: HashMap<String, usize>,
    /// Symbols indexed by address
    symbols_by_address: HashMap<u64, Vec<usize>>,
    /// Global type table from DWARF (wrapped in Arc for sharing)
    type_table: SharedTypeTable,
    /// Diagnostic statistics from DWARF parsing
    diagnostics: DwarfDiagnostics,
}

impl ElfInfo {
    /// Create a new empty ElfInfo
    fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            entry_point: 0,
            is_64bit: false,
            is_little_endian: true,
            machine: String::new(),
            symbols: Vec::new(),
            symbols_by_name: HashMap::new(),
            symbols_by_address: HashMap::new(),
            type_table: Arc::new(TypeTable::new()),
            diagnostics: DwarfDiagnostics::default(),
        }
    }

    /// Set the type table (used during parsing)
    fn set_type_table(&mut self, table: TypeTable) {
        self.type_table = Arc::new(table);
    }

    /// Set the diagnostics (used during parsing)
    fn set_diagnostics(&mut self, diagnostics: DwarfDiagnostics) {
        self.diagnostics = diagnostics;
    }

    /// Get a reference to the shared type table
    pub fn type_table(&self) -> &SharedTypeTable {
        &self.type_table
    }

    /// Get DWARF parsing diagnostics
    pub fn get_diagnostics(&self) -> &DwarfDiagnostics {
        &self.diagnostics
    }

    /// Get the status of a specific symbol
    pub fn get_symbol_status<'a>(&self, symbol: &'a SymbolInfo) -> Option<&'a VariableStatus> {
        symbol.status.as_ref()
    }

    /// Add a symbol to the index
    fn add_symbol(&mut self, symbol: SymbolInfo) {
        let index = self.symbols.len();

        // Index by display name
        self.symbols_by_name
            .insert(symbol.display_name.clone(), index);

        // Also index by mangled name if different
        if symbol.mangled_name != symbol.display_name {
            self.symbols_by_name
                .insert(symbol.mangled_name.clone(), index);
        }

        // Full demangled path is the stable identity across rebuilds
        if symbol.demangled_name != symbol.display_name {
            self.symbols_by_name
                .insert(symbol.demangled_name.clone(), index);
        }

        // Index by address
        self.symbols_by_address
            .entry(symbol.address)
            .or_default()
            .push(index);

        self.symbols.push(symbol);
    }

    /// Find a symbol by name
    pub fn find_symbol(&self, name: &str) -> Option<&SymbolInfo> {
        self.symbols_by_name
            .get(name)
            .map(|&idx| &self.symbols[idx])
    }

    /// Find symbols at a specific address
    pub fn find_symbols_at_address(&self, address: u64) -> Vec<&SymbolInfo> {
        self.symbols_by_address
            .get(&address)
            .map(|indices| indices.iter().map(|&idx| &self.symbols[idx]).collect())
            .unwrap_or_default()
    }

    /// Get all variable symbols
    pub fn get_variables(&self) -> Vec<&SymbolInfo> {
        self.symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Variable)
            .collect()
    }

    /// Get all function symbols
    pub fn get_functions(&self) -> Vec<&SymbolInfo> {
        self.symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Function)
            .collect()
    }

    /// Search for symbols matching a query
    pub fn search_symbols(&self, query: &str) -> Vec<&SymbolInfo> {
        self.symbols.iter().filter(|s| s.matches(query)).collect()
    }

    /// Search for variables matching a query
    pub fn search_variables(&self, query: &str) -> Vec<&SymbolInfo> {
        self.symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Variable)
            .filter(|s| query.is_empty() || s.matches(query))
            .collect()
    }

    /// Get the number of variable symbols
    pub fn variable_count(&self) -> usize {
        self.symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Variable)
            .count()
    }

    /// Get the number of function symbols
    pub fn function_count(&self) -> usize {
        self.symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Function)
            .count()
    }

    /// Get a TypeHandle for a symbol
    pub fn symbol_type_handle(&self, symbol: &SymbolInfo) -> Option<TypeHandle> {
        symbol.type_handle(&self.type_table)
    }

    /// Infer variable type for a symbol using type table
    pub fn infer_variable_type_for_symbol(&self, symbol: &SymbolInfo) -> VariableType {
        if let Some(handle) = self.symbol_type_handle(symbol) {
            handle.to_variable_type()
        } else {
            symbol.infer_variable_type()
        }
    }

    /// Check if a symbol is expandable (has members)
    pub fn is_symbol_expandable(&self, symbol: &SymbolInfo) -> bool {
        self.symbol_type_handle(symbol)
            .is_some_and(|h| h.is_expandable())
    }

    /// Check if a symbol is complex (struct, union, array)
    pub fn is_symbol_complex(&self, symbol: &SymbolInfo) -> bool {
        self.symbol_type_handle(symbol)
            .is_some_and(|h| h.is_struct_or_union())
    }

    /// Check if a symbol can be added as a watchable variable
    pub fn is_symbol_addable(&self, symbol: &SymbolInfo) -> bool {
        self.symbol_type_handle(symbol)
            .is_none_or(|h| h.is_addable())
    }

    /// Get the type name for a symbol
    pub fn get_symbol_type_name(&self, symbol: &SymbolInfo) -> String {
        self.symbol_type_handle(symbol)
            .map(|h| h.type_name())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Get the members of a symbol's type
    pub fn get_symbol_members(&self, symbol: &SymbolInfo) -> Option<&[MemberDef]> {
        symbol
            .type_id
            .and_then(|id| self.type_table.get_members(id))
    }

    /// Get the underlying TypeHandle for a symbol (strips typedefs, qualifiers)
    pub fn get_symbol_underlying_type(&self, symbol: &SymbolInfo) -> Option<TypeHandle> {
        self.symbol_type_handle(symbol).map(|h| h.underlying())
    }
}

/// Demangle a symbol name (Rust v0 and legacy, then C++).
///
/// Rust goes first: legacy Rust symbols (`_ZN...17h<hash>E`) are also valid Itanium
/// names, and the C++ demangler would keep the `::h<hash>` segment. Rust output uses
/// the alternate form, which drops the crate disambiguator (`foo[1a2b...]`) and the
/// legacy hash, so the path stays stable across rebuilds.
pub fn demangle_symbol(mangled: &str) -> String {
    if let Ok(demangled) = rustc_demangle::try_demangle(mangled) {
        return format!("{demangled:#}");
    }

    if let Ok(symbol) = CppSymbol::new(mangled) {
        if let Ok(demangled) = symbol.demangle() {
            return demangled;
        }
    }

    mangled.to_string()
}

/// Extract a short name from a demangled symbol (e.g., just the function/variable name)
fn extract_short_name(demangled: &str) -> String {
    // For C++ names like "namespace::Class::method(args)", extract "method"
    // For Rust names like "crate::module::function", extract "function"

    // Remove template parameters and function arguments for cleaner display
    let cleaned = remove_nested(demangled, '<', '>');
    let cleaned = remove_nested(&cleaned, '(', ')');

    // Split by :: and take the last part
    if let Some(last) = cleaned.rsplit("::").next() {
        // Clean up any remaining spaces or qualifiers
        let name = last.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }

    demangled.to_string()
}

/// ARM/AArch64/RISC-V mapping symbols (`$a`, `$t`, `$d`, `$x`, optionally with a
/// `.suffix`) mark code/data regions, not objects. They share addresses with real
/// variables and would otherwise pick up their DWARF types.
fn is_mapping_symbol(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('$') else {
        return false;
    };
    let kind = rest.split('.').next().unwrap_or("");
    matches!(kind, "a" | "t" | "d" | "x")
}

/// rustc/LLVM anonymous allocations (`.Lanon.<hash>.<n>`, `anon.<hash>.<n>.llvm.<n>`):
/// string literals, vtables and promoted constants in flash, never watch targets.
fn is_anonymous_constant(name: &str) -> bool {
    name.strip_prefix(".L").unwrap_or(name).starts_with("anon.")
}

/// Remove nested delimiters from a string
fn remove_nested(s: &str, open: char, close: char) -> String {
    let mut result = String::new();
    let mut depth: i32 = 0;
    for c in s.chars() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.saturating_sub(1);
        } else if depth == 0 {
            result.push(c);
        }
    }
    result
}

/// ELF parser for extracting symbols and type information
pub struct ElfParser;

impl ElfParser {
    /// Parse an ELF file from a path
    pub fn parse<P: AsRef<Path>>(path: P) -> Result<ElfInfo> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;

        Self::parse_bytes(&data, path.to_string_lossy().as_ref())
    }

    /// Parse ELF data from bytes
    pub fn parse_bytes(data: &[u8], path: &str) -> Result<ElfInfo> {
        let file = object::File::parse(data).map_err(|e| Error::Elf(e.to_string()))?;

        let mut info = ElfInfo::new(path);

        // Basic ELF info
        info.entry_point = file.entry();
        info.is_64bit = file.is_64();
        info.is_little_endian = file.is_little_endian();
        info.machine = format!("{:?}", file.architecture());

        // Parse symbols from symbol table
        for symbol in file.symbols() {
            if let Some(sym_info) = Self::parse_symbol(&symbol, &file) {
                info.add_symbol(sym_info);
            }
        }

        // Also check dynamic symbols
        for symbol in file.dynamic_symbols() {
            if let Some(sym_info) = Self::parse_symbol(&symbol, &file) {
                // Only add if not already present
                if info.find_symbol(&sym_info.mangled_name).is_none() {
                    info.add_symbol(sym_info);
                }
            }
        }

        // Try to parse DWARF debug info
        if let Err(e) = Self::parse_dwarf(data, &file, &mut info) {
            tracing::debug!("DWARF parsing failed or not available: {}", e);
        }

        Self::assign_sections(&file, &mut info);

        Ok(info)
    }

    /// Place every symbol in the section containing its address. Symbols added from
    /// DWARF have no section index, and a symbol's own section may be non-allocated
    /// metadata (`.defmt`), so writability comes from the section headers.
    fn assign_sections(file: &object::File, info: &mut ElfInfo) {
        let sections: Vec<(u64, u64, String, bool)> = file
            .sections()
            .filter_map(|s| {
                let object::SectionFlags::Elf { sh_flags, .. } = s.flags() else {
                    return None;
                };
                let alloc = sh_flags.0 & object::elf::SHF_ALLOC.0 != 0;
                let write = sh_flags.0 & object::elf::SHF_WRITE.0 != 0;
                alloc.then(|| {
                    (
                        s.address(),
                        s.size(),
                        s.name().unwrap_or("").to_string(),
                        write,
                    )
                })
            })
            .collect();

        for symbol in &mut info.symbols {
            let containing = sections.iter().find(|(addr, size, _, _)| {
                symbol.address >= *addr && symbol.address < addr + (*size).max(1)
            });
            match containing {
                Some((_, _, name, write)) => {
                    if symbol.section.is_empty() {
                        symbol.section = name.clone();
                    }
                    symbol.writable = *write;
                }
                None => symbol.writable = false,
            }
        }
    }

    /// Parse DWARF debug information and populate the type table
    fn parse_dwarf(data: &[u8], _file: &object::File, info: &mut ElfInfo) -> Result<()> {
        // Use the new DwarfParser to parse types and symbols
        let result = DwarfParser::parse_bytes(data).map_err(Error::Dwarf)?;

        // Set the type table and diagnostics
        info.set_type_table(result.type_table);
        info.set_diagnostics(result.diagnostics);

        // Map parsed DWARF symbols to existing symbols by address
        // Build a lookup from address to parsed symbol for efficient matching
        // Include address 0 symbols since they may be valid on embedded targets
        let dwarf_symbols_by_addr: HashMap<u64, &ParsedSymbol> = result
            .symbols
            .iter()
            .filter(|s| s.status.is_readable())
            .map(|s| (s.address, s))
            .collect();

        // Also build a lookup by name for symbols without matching addresses
        // Include both the name and mangled_name for better matching
        let mut dwarf_symbols_by_name: HashMap<&str, &ParsedSymbol> = HashMap::new();
        for sym in &result.symbols {
            dwarf_symbols_by_name.insert(sym.name.as_str(), sym);
            if let Some(ref mangled) = sym.mangled_name {
                dwarf_symbols_by_name.insert(mangled.as_str(), sym);
            }
        }

        let mut matched_by_addr = 0;
        let mut matched_by_name = 0;

        // Update existing symbols with type information and status from DWARF
        for symbol in &mut info.symbols {
            // First try to match by address
            if let Some(dwarf_sym) = dwarf_symbols_by_addr.get(&symbol.address) {
                symbol.type_id = Some(dwarf_sym.type_id);
                symbol.status = Some(dwarf_sym.status.clone());
                // Update size if DWARF has better info
                if dwarf_sym.size > 0 && symbol.size == 0 {
                    symbol.size = dwarf_sym.size;
                }
                matched_by_addr += 1;
                continue;
            }

            // Fall back to matching by name (try display name, demangled, then mangled)
            let matched = dwarf_symbols_by_name
                .get(symbol.display_name.as_str())
                .or_else(|| dwarf_symbols_by_name.get(symbol.demangled_name.as_str()))
                .or_else(|| dwarf_symbols_by_name.get(symbol.mangled_name.as_str()));

            if let Some(dwarf_sym) = matched {
                symbol.type_id = Some(dwarf_sym.type_id);
                symbol.status = Some(dwarf_sym.status.clone());
                if dwarf_sym.size > 0 && symbol.size == 0 {
                    symbol.size = dwarf_sym.size;
                }
                matched_by_name += 1;
                continue;
            }

            // Fall back to the name-to-type mapping (for variables without addresses)
            let type_id = result
                .name_to_type
                .get(&symbol.mangled_name)
                .or_else(|| result.name_to_type.get(&symbol.demangled_name))
                .or_else(|| result.name_to_type.get(&symbol.display_name));

            if let Some(&tid) = type_id {
                symbol.type_id = Some(tid);
                // Try to get size from the type if not set
                if symbol.size == 0 {
                    if let Some(size) = info.type_table.type_size(tid) {
                        symbol.size = size;
                    }
                }
                matched_by_name += 1;
            }
        }

        // Add any DWARF-discovered symbols that aren't in the symbol table
        // Only add symbols that are readable (have valid addresses)
        let mut _added_from_dwarf = 0;
        for dwarf_sym in &result.symbols {
            // Skip symbols that aren't readable
            if !dwarf_sym.status.is_readable() {
                continue;
            }

            // Check if we already have this symbol
            let exists = info.symbols.iter().any(|s| s.address == dwarf_sym.address);
            if exists {
                continue;
            }

            // Create a new symbol from DWARF info
            let mangled_name = dwarf_sym
                .mangled_name
                .clone()
                .unwrap_or_else(|| dwarf_sym.name.clone());
            let demangled_name = demangle_symbol(&mangled_name);
            let display_name = extract_short_name(&demangled_name);

            info.add_symbol(SymbolInfo {
                mangled_name,
                demangled_name,
                display_name,
                address: dwarf_sym.address,
                size: dwarf_sym.size,
                symbol_type: SymbolType::Variable,
                section: String::new(),
                is_global: dwarf_sym.is_global,
                writable: false,
                type_id: Some(dwarf_sym.type_id),
                status: Some(dwarf_sym.status.clone()),
            });
            _added_from_dwarf += 1;
        }

        // Count variables with and without type info
        let vars_with_type = info
            .symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Variable && s.type_id.is_some())
            .count();
        let total_vars = info
            .symbols
            .iter()
            .filter(|s| s.symbol_type == SymbolType::Variable)
            .count();

        let stats = info.type_table.stats();
        tracing::info!(
            "DWARF: {} types ({} structs) | Variables: {}/{} have type info (matched {} by addr, {} by name)",
            stats.total_types,
            stats.structs,
            vars_with_type,
            total_vars,
            matched_by_addr,
            matched_by_name,
        );

        // Log unmatched symbols for debugging
        let unmatched: Vec<_> = info
            .symbols
            .iter()
            .filter(|s| s.type_id.is_none() && s.symbol_type == SymbolType::Variable)
            .take(5)
            .collect();
        if !unmatched.is_empty() {
            tracing::debug!(
                "Sample unmatched variables: {:?}",
                unmatched
                    .iter()
                    .map(|s| (&s.display_name, s.address))
                    .collect::<Vec<_>>()
            );
        }

        Ok(())
    }

    /// Parse a single symbol from the symbol table
    fn parse_symbol(symbol: &object::Symbol, file: &object::File) -> Option<SymbolInfo> {
        let name = symbol.name().ok()?;
        if name.is_empty() || is_mapping_symbol(name) || is_anonymous_constant(name) {
            return None;
        }

        let address = symbol.address();

        // Skip symbols with no address (undefined symbols)
        if address == 0 && symbol.kind() != SymbolKind::Unknown {
            // Allow address 0 for some symbol types
            if symbol.kind() == SymbolKind::Section || symbol.kind() == SymbolKind::File {
                return None;
            }
        }

        let size = symbol.size();
        let is_global = symbol.is_global();

        let symbol_type = match symbol.kind() {
            SymbolKind::Data => SymbolType::Variable,
            SymbolKind::Text => SymbolType::Function,
            SymbolKind::Section => SymbolType::Section,
            SymbolKind::File => SymbolType::File,
            _ => SymbolType::Other,
        };

        // Get section name
        let section = symbol
            .section_index()
            .and_then(|idx| file.section_by_index(idx).ok())
            .and_then(|s| s.name().ok())
            .unwrap_or("")
            .to_string();

        // Skip certain sections
        if section.starts_with(".debug") || section.starts_with(".comment") {
            return None;
        }

        let mangled_name = name.to_string();
        let demangled_name = demangle_symbol(&mangled_name);
        let display_name = extract_short_name(&demangled_name);

        Some(SymbolInfo {
            mangled_name,
            demangled_name,
            display_name,
            address,
            size,
            symbol_type,
            section,
            is_global,
            writable: false,
            type_id: None,
            status: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ARM_ELF: &[u8] = include_bytes!("../tests/fixtures/test_arm.elf");
    const TEST_STRUCT_ELF: &[u8] = include_bytes!("../tests/fixtures/test_struct.elf");
    const TEST_CPP_ELF: &[u8] = include_bytes!("../tests/fixtures/test_cpp.elf");

    #[test]
    fn symbols_merge_with_dwarf_types() {
        let elf = ElfParser::parse_bytes(TEST_ARM_ELF, "test_arm.elf").unwrap();
        assert!(elf.is_little_endian);
        assert!(!elf.is_64bit);

        let counter = elf.find_symbol("global_counter").expect("global_counter");
        assert_eq!(counter.symbol_type, SymbolType::Variable);
        assert!(counter.type_id.is_some(), "DWARF type not merged");
        assert!(counter.is_readable());
        assert_eq!(
            elf.infer_variable_type_for_symbol(counter),
            VariableType::U32
        );
        assert!(elf
            .find_symbols_at_address(counter.address)
            .iter()
            .any(|s| s.display_name == "global_counter"));
    }

    #[test]
    fn struct_symbol_exposes_members() {
        let elf = ElfParser::parse_bytes(TEST_STRUCT_ELF, "test_struct.elf").unwrap();
        let sym = elf.find_symbol("sensor_struct").expect("sensor_struct");
        assert!(elf.is_symbol_complex(sym));
        let names: Vec<_> = elf
            .get_symbol_members(sym)
            .unwrap()
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert!(names.contains(&"value"), "{names:?}");
    }

    #[test]
    fn cpp_symbols_are_demangled() {
        let elf = ElfParser::parse_bytes(TEST_CPP_ELF, "test_cpp.elf").unwrap();
        let mangled: Vec<_> = elf
            .symbols
            .iter()
            .filter(|s| s.mangled_name.starts_with("_Z"))
            .collect();
        assert!(!mangled.is_empty());
        assert!(mangled.iter().all(|s| !s.demangled_name.starts_with("_Z")));
    }

    #[test]
    fn demangle_handles_cpp_rust_and_plain() {
        assert_eq!(demangle_symbol("_ZN3foo3barE"), "foo::bar");
        // Legacy Rust: hash segment dropped
        assert_eq!(
            demangle_symbol("_ZN4core3fmt5write17h0123456789abcdefE"),
            "core::fmt::write"
        );
        // Rust v0: crate disambiguator dropped
        assert_eq!(
            demangle_symbol("_RNvNtCs54eYuXc1AiR_24balance_infantry_chassis9transport3IMU"),
            "balance_infantry_chassis::transport::IMU"
        );
        // LLVM ThinLTO suffix is not part of the path
        assert_eq!(
            demangle_symbol(
                "_RNvNtCs4WTe2uLtwMp_13embassy_stm323rcc11CLOCK_FREQS.llvm.13985685041032945125"
            ),
            "embassy_stm32::rcc::CLOCK_FREQS"
        );
        assert_eq!(demangle_symbol("_ZN3foo3barEi"), "foo::bar(int)");
        assert_eq!(demangle_symbol("global_counter"), "global_counter");
        assert_eq!(extract_short_name("ns::Class<int>::get(int)"), "get");
    }

    #[test]
    fn mapping_symbols_are_skipped() {
        assert!(is_mapping_symbol("$d"));
        assert!(is_mapping_symbol("$t.1"));
        assert!(!is_mapping_symbol("$dollar"));
        assert!(!is_mapping_symbol("d"));
        assert!(is_anonymous_constant(
            ".Lanon.10c5378abbece42ad2b99191221dc5f8.1"
        ));
        assert!(is_anonymous_constant(
            "anon.4f546bd6774f14d22f661135736587c5.1.llvm.17422244049030701297"
        ));
        assert!(!is_anonymous_constant("anonymous_counter"));
        let elf = ElfParser::parse_bytes(TEST_ARM_ELF, "test_arm.elf").unwrap();
        assert!(elf.symbols.iter().all(|s| !s.mangled_name.starts_with('$')));
    }

    #[test]
    fn garbage_is_an_elf_error() {
        assert!(matches!(
            ElfParser::parse_bytes(b"not an elf", "x"),
            Err(Error::Elf(_))
        ));
    }
}
