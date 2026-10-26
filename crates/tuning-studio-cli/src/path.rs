//! Text paths to DWARF nodes: `CONTROL.yaw.motor.speed_rpm`, `buffer[3]`,
//! `data_ptr.*`, `app::MODE#Some.__0`.

use anyhow::{Result, anyhow, bail};
use studio_dwarf::{ElfInfo, NodeRef, Step, SymbolNode, TypeId, VariableType, tree};

use crate::memory::Memory;

/// What a command reads or writes.
pub enum Location {
    Node(Box<SymbolNode>),
    /// An address with no ELF type information
    Raw {
        address: u64,
        scalar: VariableType,
    },
}

/// Parse `text` as a symbol path, or as an address with an optional type and path:
/// `0x24000070`, `0x24000070:f32`, `0x20000038:Axis.gains.2`. An address without a
/// type reads the variable that starts there, or a `u32`. Types other than the
/// primitives come from the ELF, by the name it prints for them.
pub fn locate(elf: Option<&ElfInfo>, memory: &mut dyn Memory, text: &str) -> Result<Location> {
    let Some((address, rest)) = split_address(text)? else {
        let elf = elf.ok_or_else(|| anyhow!("pass --elf to use symbol names"))?;
        let node = parse_path(elf, memory, text)?;
        return Ok(Location::Node(Box::new(describe(elf, memory, &node)?)));
    };
    if rest.is_empty() {
        if let Some(elf) = elf {
            // The first variable there that has type information
            for symbol in elf.find_symbols_at_address(address) {
                let node = NodeRef::root(symbol.demangled_name.clone());
                if let Ok(node) = describe(elf, memory, &node) {
                    return Ok(Location::Node(Box::new(node)));
                }
            }
        }
        return Ok(Location::Raw {
            address,
            scalar: VariableType::U32,
        });
    }
    let typed = rest
        .strip_prefix(':')
        .ok_or_else(|| anyhow!("expected `:type` after the address in `{text}`"))?;
    let (type_text, steps) = split_type(typed);
    if steps.is_empty()
        && let Ok(scalar) = parse_scalar(type_text)
    {
        return Ok(Location::Raw { address, scalar });
    }
    let elf = elf.ok_or_else(|| anyhow!("pass --elf to use the type `{type_text}`"))?;
    let type_id = find_type(elf, type_text)?;
    let root = NodeRef::typed(format!("{address:#010x}:{type_text}"), address, type_id);
    let node = parse_steps(elf, memory, root, steps, text)?;
    Ok(Location::Node(Box::new(describe(elf, memory, &node)?)))
}

pub fn describe(elf: &ElfInfo, memory: &mut dyn Memory, node: &NodeRef) -> Result<SymbolNode> {
    tree::node_with_memory(elf, node, &mut |address, len| {
        memory.word(address, len).map_err(|e| format!("{e:#}"))
    })
    .map_err(|e| anyhow!("{e}"))
}

/// A leading decimal or `0x` hex address, and the text after it.
fn split_address(text: &str) -> Result<Option<(u64, &str)>> {
    let (digits, radix) = match text.strip_prefix("0x") {
        Some(hex) => (hex, 16),
        None => (text, 10),
    };
    let end = digits
        .find(|c: char| !(c.is_digit(radix) || c == '_'))
        .unwrap_or(digits.len());
    if end == 0 || (radix == 10 && digits[end..].starts_with(|c: char| c.is_alphabetic())) {
        return Ok(None);
    }
    // The target is 32-bit, so a wider address names nothing.
    let address = u32::from_str_radix(&digits[..end].replace('_', ""), radix)
        .map_err(|e| anyhow!("bad 32-bit address in `{text}`: {e}"))?;
    Ok(Some((u64::from(address), &digits[end..])))
}

/// Split `Axis.gains` into the type and the path after it. A type may contain `.`,
/// `#` or `[` only inside `<>`, `()` or `[]`, and may start with `[`, as `[u16; 300]` does.
fn split_type(text: &str) -> (&str, &str) {
    let mut depth = 0i32;
    for (i, c) in text.char_indices() {
        match c {
            '[' if depth == 0 && i > 0 => return text.split_at(i),
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            '.' | '#' if depth == 0 => return text.split_at(i),
            _ => {}
        }
    }
    (text, "")
}

/// The type whose name, as the ELF prints it, is `name`, or ends in `::name`.
/// Each compile unit has its own copy of a type, so only distinct names conflict.
fn find_type(elf: &ElfInfo, name: &str) -> Result<TypeId> {
    let table = elf.type_table();
    let suffix = format!("::{name}");
    let mut matches: Vec<(String, TypeId)> = table
        .iter()
        .filter(|&(id, _)| table.type_size(id).is_some_and(|size| size > 0))
        .map(|(id, _)| (table.type_name(id), id))
        .filter(|(full, _)| full == name || full.ends_with(&suffix))
        .collect();
    if let Some(exact) = matches.iter().find(|(full, _)| full == name) {
        return Ok(exact.1);
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches.dedup_by(|a, b| a.0 == b.0);
    match matches.as_slice() {
        [] => bail!("no type named `{name}` in the ELF"),
        [(_, id)] => Ok(*id),
        _ => {
            let names: Vec<_> = matches.iter().map(|(full, _)| full.as_str()).collect();
            bail!(
                "`{name}` matches {}; use more of the path",
                names.join(", ")
            )
        }
    }
}

fn parse_scalar(name: &str) -> Result<VariableType> {
    Ok(match name {
        "u8" => VariableType::U8,
        "u16" => VariableType::U16,
        "u32" => VariableType::U32,
        "u64" => VariableType::U64,
        "i8" => VariableType::I8,
        "i16" => VariableType::I16,
        "i32" => VariableType::I32,
        "i64" => VariableType::I64,
        "f32" => VariableType::F32,
        "f64" => VariableType::F64,
        "bool" => VariableType::Bool,
        _ => bail!("unknown type `{name}`; use u8 to u64, i8 to i64, f32, f64 or bool"),
    })
}

fn parse_path(elf: &ElfInfo, memory: &mut dyn Memory, text: &str) -> Result<NodeRef> {
    // The symbol is the longest prefix, ending before a `.`, `[` or `#`, that names one.
    // Symbol names keep their `::` and anything inside `<>`, `()` or `{}`.
    let mut depth = 0i32;
    let mut ends = Vec::new();
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' | '{' => depth += 1,
            '>' | ')' | '}' => depth -= 1,
            '.' | '[' | '#' if depth == 0 => ends.push(i),
            _ => {}
        }
    }
    ends.push(text.len());
    let mut found = None;
    for &end in ends.iter().rev() {
        if let Some(symbol) = find_symbol(elf, &text[..end])? {
            found = Some((symbol, &text[end..]));
            break;
        }
    }
    let (symbol, rest) = found.ok_or_else(|| anyhow!("no symbol named `{}`", &text[..ends[0]]))?;

    parse_steps(elf, memory, NodeRef::root(symbol), rest, text)
}

/// Follow `rest`, such as `.yaw[1].*`, from `node`. `text` is the whole path, for errors.
fn parse_steps(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    mut node: NodeRef,
    mut rest: &str,
    text: &str,
) -> Result<NodeRef> {
    while !rest.is_empty() {
        let (step, tail) = if let Some(tail) = rest.strip_prefix(".*") {
            (Step::Deref, tail)
        } else if let Some(tail) = rest.strip_prefix('.') {
            let (name, tail) = split_name(tail);
            (member(elf, memory, &node, name)?, tail)
        } else if let Some(tail) = rest.strip_prefix('#') {
            let (name, tail) = split_name(tail);
            (Step::Variant(name.to_string()), tail)
        } else if let Some(tail) = rest.strip_prefix('[') {
            let (index, tail) = tail
                .split_once(']')
                .ok_or_else(|| anyhow!("missing `]` in `{text}`"))?;
            // `[task 0]` is a slot of an Embassy task pool, seen as a `TaskStorage`.
            let (slot, index) = match index.strip_prefix("task ") {
                Some(slot) => (true, slot),
                None => (false, index),
            };
            let index: u64 = index
                .parse()
                .map_err(|_| anyhow!("bad index `{index}` in `{text}`"))?;
            if slot {
                (Step::Task(index), tail)
            } else if describe(elf, memory, &node)?.sequence {
                (Step::SliceIndex(index), tail)
            } else {
                (Step::Index(index), tail)
            }
        } else {
            bail!("cannot parse `{rest}` in `{text}`");
        };
        if let Step::Member(name) | Step::Variant(name) = &step
            && name.is_empty()
        {
            bail!("missing name in `{text}`");
        }
        node = node.child(step);
        describe(elf, memory, &node)?;
        rest = tail;
    }
    Ok(node)
}

/// The full name of the variable `name` refers to: its full or mangled name, or the
/// end of its full name after a `::`, such as `CONTROL` or `app::CONTROL`.
fn find_symbol(elf: &ElfInfo, name: &str) -> Result<Option<String>> {
    if let Some(symbol) = elf.find_symbol(name)
        && (symbol.demangled_name == name || symbol.mangled_name == name)
    {
        return Ok(Some(symbol.demangled_name.clone()));
    }
    let suffix = format!("::{name}");
    let mut matches: Vec<&str> = elf
        .get_variables()
        .into_iter()
        .map(|s| s.demangled_name.as_str())
        .filter(|full| full.ends_with(&suffix))
        .collect();
    matches.sort_unstable();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.to_string())),
        _ => bail!(
            "`{name}` matches {}; use more of the path",
            matches.join(", ")
        ),
    }
}

fn split_name(text: &str) -> (&str, &str) {
    let end = text.find(['.', '[', '#']).unwrap_or(text.len());
    text.split_at(end)
}

/// Rust names tuple fields `__0`, `__1`; accept `.0` for them.
fn member(elf: &ElfInfo, memory: &mut dyn Memory, node: &NodeRef, name: &str) -> Result<Step> {
    let step = Step::Member(name.to_string());
    if name.bytes().all(|b| b.is_ascii_digit())
        && describe(elf, memory, &node.child(step.clone())).is_err()
    {
        return Ok(Step::Member(format!("__{name}")));
    }
    Ok(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Fake;
    use studio_dwarf::ElfParser;

    fn fixture(name: &str) -> ElfInfo {
        ElfParser::parse(format!(
            "{}/../studio-dwarf/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn path(elf: &ElfInfo, text: &str) -> String {
        let mut memory = Fake::new(0x2000_0000, vec![0; 0x2000]);
        match locate(Some(elf), &mut memory, text).unwrap() {
            Location::Node(node) => node.path,
            Location::Raw { address, scalar } => format!("{address:#x}:{scalar}"),
        }
    }

    #[test]
    fn addresses_are_32_bit() {
        let (address, rest) = split_address("0xFFFF_FFFF:u32").unwrap().unwrap();
        assert_eq!((address, rest), (0xFFFF_FFFF, ":u32"));
        assert!(split_address("0x1_0000_0000:u32").is_err());
        assert!(split_address("4294967296").is_err());
    }

    #[test]
    fn parses_members_and_indices() {
        let elf = fixture("test_struct.elf");
        assert_eq!(
            path(&elf, "device_config.sensor.value"),
            "device_config.sensor.value"
        );
        assert_eq!(path(&elf, "buffer[7]"), "buffer[7]");
    }

    #[test]
    fn parses_addresses() {
        let elf = fixture("test_struct.elf");
        let address = elf.find_symbol("buffer").unwrap().address;
        assert_eq!(path(&elf, &format!("{address:#x}")), "buffer");
        assert_eq!(path(&elf, "0x2000_0004:f32"), "0x20000004:f32");
        assert_eq!(path(&elf, "0x30000000"), "0x30000000:u32");
    }

    #[test]
    fn rejects_unknown_names() {
        let elf = fixture("test_struct.elf");
        let mut memory = Fake::new(0, vec![]);
        for text in ["nothing", "device_config.missing", "buffer[8]", "buffer[x]"] {
            assert!(locate(Some(&elf), &mut memory, text).is_err(), "{text}");
        }
    }

    #[test]
    fn parses_typed_addresses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        let mut memory = Fake::from_elf(path);
        let robot = elf
            .find_symbol("type_lab::aggregates::ROBOT")
            .unwrap()
            .address;
        for (text, expected) in [
            (
                format!("{robot:#x}:Robot<4>"),
                format!("{robot:#010x}:Robot<4>"),
            ),
            (
                format!("{robot:#x}:Robot<4>.axes[1].gains.2"),
                format!("{robot:#010x}:Robot<4>.axes[1].gains.__2"),
            ),
            (
                format!("{robot:#x}:Axis.velocity"),
                format!("{robot:#010x}:Axis.velocity"),
            ),
            (
                format!("{robot:#x}:[i16; 4][3]"),
                format!("{robot:#010x}:[i16; 4][3]"),
            ),
        ] {
            let Location::Node(node) = locate(Some(&elf), &mut memory, &text).unwrap() else {
                panic!("{text} is not a node");
            };
            assert_eq!(node.path, expected);
        }
        let mut memory = Fake::new(0, vec![]);
        for text in [
            "0x20000000:NoSuchType",
            "0x20000000:Axis.missing",
            "0x20000000Axis",
        ] {
            assert!(locate(Some(&elf), &mut memory, text).is_err(), "{text}");
        }
        assert!(locate(None, &mut memory, "0x20000000:Axis").is_err());
    }
}

#[cfg(test)]
mod task_tests {
    use super::*;
    use crate::memory::Fake;
    use studio_dwarf::ElfParser;

    #[test]
    fn parses_task_slots() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/select_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        let mut memory = Fake::from_elf(path);
        let text = "select_lab::waves::POOL[task 0].future";
        let Location::Node(node) = locate(Some(&elf), &mut memory, text).unwrap() else {
            panic!("{text} is not a node");
        };
        assert_eq!(node.path, text);
        assert!(
            node.type_name.contains("async_fn_env"),
            "{}",
            node.type_name
        );
        for bad in [
            "select_lab::waves::POOL[task 1]",
            "select_lab::waves::POOL[task x]",
        ] {
            assert!(locate(Some(&elf), &mut memory, bad).is_err(), "{bad}");
        }
    }
}
