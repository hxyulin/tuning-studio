//! Decode values for printing, and encode values for writing.

use anyhow::{Result, anyhow, bail};
use std::fmt::Write as _;
use studio_dwarf::tree::{self, NodeKind, Step, SymbolNode};
use studio_dwarf::{ElfInfo, VariableType};

use crate::memory::Memory;

/// Array elements printed before the rest are summarised.
const ARRAY_LIMIT: usize = 64;
/// Nesting printed below the requested node.
const DEPTH_LIMIT: usize = 8;
/// Larger nodes are read field by field instead of in one transfer.
const SNAPSHOT_LIMIT: u64 = 64 * 1024;

pub fn decode(scalar: VariableType, bytes: &[u8]) -> String {
    let mut raw = [0u8; 8];
    let len = bytes.len().min(8);
    raw[..len].copy_from_slice(&bytes[..len]);
    let bits = u64::from_le_bytes(raw);
    match scalar {
        VariableType::U8 | VariableType::U16 | VariableType::U32 | VariableType::U64 => {
            bits.to_string()
        }
        VariableType::I8 => (bits as i8).to_string(),
        VariableType::I16 => (bits as i16).to_string(),
        VariableType::I32 => (bits as i32).to_string(),
        VariableType::I64 => (bits as i64).to_string(),
        VariableType::F32 => format!("{:?}", f32::from_bits(bits as u32)),
        VariableType::F64 => format!("{:?}", f64::from_bits(bits)),
        VariableType::Bool => match bits {
            0 => "false".into(),
            1 => "true".into(),
            other => format!("invalid bool ({other})"),
        },
        VariableType::Raw(_) => bytes.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

/// Little-endian bytes of `text` as `scalar`. Integers accept `0x` hex and `_`.
pub fn encode(scalar: VariableType, text: &str) -> Result<Vec<u8>> {
    let int = |min: i128, max: i128, size: usize| -> Result<Vec<u8>> {
        let clean = text.replace('_', "");
        let value = match clean.strip_prefix("0x") {
            Some(hex) => i128::from_str_radix(hex, 16),
            None => clean.parse(),
        }
        .map_err(|_| anyhow!("`{text}` is not an integer"))?;
        if !(min..=max).contains(&value) {
            bail!("{value} is outside {scalar}'s range {min} to {max}");
        }
        Ok(value.to_le_bytes()[..size].to_vec())
    };
    let float = || -> Result<f64> {
        text.parse()
            .map_err(|_| anyhow!("`{text}` is not a number"))
    };
    match scalar {
        VariableType::U8 => int(0, u8::MAX.into(), 1),
        VariableType::U16 => int(0, u16::MAX.into(), 2),
        VariableType::U32 => int(0, u32::MAX.into(), 4),
        VariableType::U64 => int(0, u64::MAX.into(), 8),
        VariableType::I8 => int(i8::MIN.into(), i8::MAX.into(), 1),
        VariableType::I16 => int(i16::MIN.into(), i16::MAX.into(), 2),
        VariableType::I32 => int(i32::MIN.into(), i32::MAX.into(), 4),
        VariableType::I64 => int(i64::MIN.into(), i64::MAX.into(), 8),
        VariableType::F32 => {
            let value = float()?;
            if value.is_finite() && (value as f32).is_infinite() {
                bail!("{value} is outside f32's range");
            }
            Ok((value as f32).to_le_bytes().to_vec())
        }
        VariableType::F64 => Ok(float()?.to_le_bytes().to_vec()),
        VariableType::Bool => match text {
            "true" | "1" => Ok(vec![1]),
            "false" | "0" => Ok(vec![0]),
            _ => bail!("`{text}` is not true, false, 1 or 0"),
        },
        VariableType::Raw(size) => bail!("cannot write a {size}-byte value"),
    }
}

/// A variant name, or a declared variant's value, as `size` little-endian bytes.
/// Any other value would be undefined behaviour for a Rust enum.
pub fn encode_enum(variants: &[(String, i64)], size: usize, text: &str) -> Result<Vec<u8>> {
    let value = match variants.iter().find(|(name, _)| name == text) {
        Some(&(_, value)) => value,
        None => {
            let value: i64 = text
                .parse()
                .map_err(|_| anyhow!("`{text}` is not a variant: {}", names(variants)))?;
            if !variants.iter().any(|&(_, v)| v == value) {
                bail!("{value} is not a variant's value: {}", names(variants));
            }
            value
        }
    };
    Ok(value.to_le_bytes()[..size.min(8)].to_vec())
}

fn names(variants: &[(String, i64)]) -> String {
    variants
        .iter()
        .map(|(name, value)| format!("{name} ({value})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One character, such as `é`, or its code point as a number.
pub fn encode_char(text: &str) -> Result<Vec<u8>> {
    let mut chars = text.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_ascii_digit() => u32::from(c),
        _ => {
            let bytes = encode(VariableType::U32, text)?;
            let code = u32::from_le_bytes(bytes.try_into().unwrap());
            char::from_u32(code)
                .ok_or_else(|| anyhow!("{code:#x} is not a Unicode scalar value"))?;
            code
        }
    };
    Ok(code.to_le_bytes().to_vec())
}

/// The bytes [`render`] reads at once, for a node small enough to read whole.
pub fn span(node: &SymbolNode) -> Option<(u64, usize)> {
    node.size
        .filter(|&size| size > 0 && size <= SNAPSHOT_LIMIT)
        .map(|size| (node.address, size as usize))
}

/// The node and everything below it, one line per node.
pub fn render(elf: &ElfInfo, memory: &mut dyn Memory, node: &SymbolNode) -> Result<String> {
    if let Some((address, size)) = span(node) {
        memory.snapshot(address, size)?;
    }
    Ok(render_snapshot(elf, memory, node, &node.path))
}

/// As [`render`], under `label`, from bytes the caller has already snapshotted.
pub fn render_snapshot(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
    label: &str,
) -> String {
    let mut out = String::new();
    render_node(elf, memory, node, label, 0, &mut out);
    out
}

fn render_node(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
    label: &str,
    depth: usize,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    // `Atomic<u32>` rather than the `u32` inside it
    let type_name = node.wrapper.as_deref().unwrap_or(&node.type_name);
    let head = format!("{indent}{label}: {type_name}");
    let value = |memory: &mut dyn Memory| -> Result<Option<String>> {
        if !node.readable {
            bail!("{}", node.status.as_deref().unwrap_or("not readable"));
        }
        if node.bit_size.is_some() {
            bail!("bitfields are not supported yet");
        }
        // Covers strings, slice lengths, enum names and characters.
        if let Some(preview) = preview(elf, memory, node)? {
            return Ok(Some(preview.text));
        }
        match (node.kind, scalar_of(node)) {
            (NodeKind::Pointer, Some(scalar)) => {
                let bytes = memory.read(node.address, size_of(scalar))?;
                Ok(Some(format!("{:#010x}", u64_of(&bytes))))
            }
            (NodeKind::Scalar, Some(VariableType::Raw(16))) if node.type_name.ends_with("128") => {
                let bytes: [u8; 16] = memory.read(node.address, 16)?.try_into().unwrap();
                Ok(Some(match node.type_name.as_str() {
                    "i128" => i128::from_le_bytes(bytes).to_string(),
                    _ => u128::from_le_bytes(bytes).to_string(),
                }))
            }
            (NodeKind::Scalar, Some(scalar)) => {
                let bytes = memory.read(node.address, size_of(scalar))?;
                Ok(Some(decode(scalar, &bytes)))
            }
            _ => Ok(None),
        }
    };
    match value(memory) {
        Ok(Some(value)) => {
            let _ = writeln!(out, "{head} = {value}");
        }
        Ok(None) => {
            let _ = writeln!(out, "{head}");
        }
        Err(error) => {
            let _ = writeln!(out, "{head} = <{error:#}>");
            return;
        }
    }
    let text = node.sequence && node.type_name.ends_with("str");
    if text || !node.expandable || node.kind == NodeKind::Pointer {
        return;
    }
    if depth == DEPTH_LIMIT {
        let _ = writeln!(
            out,
            "{indent}  … deeper fields omitted; read {} directly",
            node.path
        );
        return;
    }
    render_children(elf, memory, node, depth + 1, out);
}

/// `node`'s children at `depth`. A tagged enum shows its active variant's fields
/// in place of the variant.
fn render_children(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
    depth: usize,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    let mut reader = |address, len| memory.word(address, len).map_err(|e| format!("{e:#}"));
    let children = match tree::children_page(elf, &node.node, 0, ARRAY_LIMIT, &mut reader, true) {
        Ok(children) => children,
        Err(error) => {
            let _ = writeln!(out, "{indent}<{error}>");
            return;
        }
    };
    // A tagged enum's payload is valid only for its active variant.
    let active = (node.kind == NodeKind::TaggedEnum)
        .then(|| preview(elf, memory, node).ok().flatten())
        .flatten()
        .and_then(|p| p.active_variant);
    for child in &children.nodes {
        match child.node.steps.last() {
            Some(Step::Discriminant) => {}
            Some(Step::Variant(name)) if active.as_ref() == Some(name) => {
                // A single-value payload, such as a niche `Option<NonZero<u32>>`, has
                // no fields of its own.
                if child.expandable {
                    render_children(elf, memory, child, depth, out);
                } else if child.scalar.is_some() {
                    // DWARF names the variant's only field `__0`; the variant name is the wrapper.
                    let mut payload = child.clone();
                    payload.wrapper = None;
                    render_node(elf, memory, &payload, "__0", depth, out);
                }
            }
            Some(Step::Variant(_)) => {}
            _ => render_node(elf, memory, child, &child.label, depth, out),
        }
    }
    let shown = children.nodes.len() as u64;
    if children.total > shown {
        let _ = writeln!(
            out,
            "{indent}… {} more; read {}[{shown}] and later directly",
            children.total - shown,
            node.path
        );
    }
}

fn preview(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<Option<tree::Preview>> {
    tree::preview(elf, &node.node, &mut |address, len| {
        memory.read(address, len).map_err(|e| format!("{e:#}"))
    })
    .map_err(|e| anyhow!("{e}"))
}

/// The type to decode after wrappers. `AtomicBool` stores its value in a `u8`.
pub fn scalar_of(node: &SymbolNode) -> Option<VariableType> {
    match node.wrapper.as_deref() {
        Some("Atomic<bool>" | "AtomicBool") => Some(VariableType::Bool),
        _ => node.scalar,
    }
}

pub fn size_of(scalar: VariableType) -> usize {
    match scalar {
        VariableType::U8 | VariableType::I8 | VariableType::Bool => 1,
        VariableType::U16 | VariableType::I16 => 2,
        VariableType::U32 | VariableType::I32 | VariableType::F32 => 4,
        VariableType::U64 | VariableType::I64 | VariableType::F64 => 8,
        VariableType::Raw(size) => size,
    }
}

fn u64_of(bytes: &[u8]) -> u64 {
    let mut raw = [0u8; 8];
    raw[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
    u64::from_le_bytes(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_exact_text() {
        assert_eq!(
            decode(VariableType::U64, &u64::MAX.to_le_bytes()),
            "18446744073709551615"
        );
        assert_eq!(
            decode(VariableType::I64, &i64::MIN.to_le_bytes()),
            "-9223372036854775808"
        );
        assert_eq!(decode(VariableType::I16, &(-2i16).to_le_bytes()), "-2");
        assert_eq!(decode(VariableType::F32, &(-0.0f32).to_le_bytes()), "-0.0");
        assert_eq!(decode(VariableType::F32, &f32::NAN.to_le_bytes()), "NaN");
        assert_eq!(
            decode(VariableType::F64, &f64::NEG_INFINITY.to_le_bytes()),
            "-inf"
        );
        assert_eq!(decode(VariableType::Bool, &[2]), "invalid bool (2)");
    }

    #[test]
    fn encodes_with_range_checks() {
        assert_eq!(
            encode(VariableType::U16, "0xBEEF").unwrap(),
            vec![0xef, 0xbe]
        );
        assert_eq!(encode(VariableType::I8, "-128").unwrap(), vec![0x80]);
        assert_eq!(
            encode(VariableType::F32, "1.5").unwrap(),
            1.5f32.to_le_bytes()
        );
        assert_eq!(encode(VariableType::Bool, "true").unwrap(), vec![1]);
        assert!(encode(VariableType::U8, "256").is_err());
        assert!(encode(VariableType::U32, "-1").is_err());
        assert!(encode(VariableType::F32, "1e39").is_err());
        assert!(encode(VariableType::Bool, "yes").is_err());
    }

    #[test]
    fn encodes_enums_and_chars() {
        let variants = [("Reverse".to_string(), -1), ("Stop".to_string(), 0)];
        assert_eq!(encode_enum(&variants, 1, "Reverse").unwrap(), vec![0xff]);
        assert_eq!(encode_enum(&variants, 1, "-1").unwrap(), vec![0xff]);
        assert_eq!(encode_enum(&variants, 1, "0").unwrap(), vec![0]);
        assert!(encode_enum(&variants, 1, "2").is_err());
        assert!(encode_enum(&variants, 1, "Forward").is_err());
        assert_eq!(encode_char("é").unwrap(), 0xe9u32.to_le_bytes());
        assert_eq!(encode_char("65").unwrap(), 65u32.to_le_bytes());
        assert!(encode_char("0xd800").is_err());
        assert!(encode_char("ab").is_err());
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::memory::Fake;
    use crate::path::{Location, locate};
    use studio_dwarf::ElfParser;

    #[test]
    fn renders_nested_struct() {
        let elf = ElfParser::parse(format!(
            "{}/../studio-dwarf/tests/fixtures/test_struct.elf",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let symbol = elf.find_symbol("device_config").unwrap();
        let mut bytes = vec![0; symbol.size as usize];
        bytes[..4].copy_from_slice(&7u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&2.5f32.to_le_bytes());
        let mut memory = Fake::new(symbol.address, bytes);
        let Location::Node(node) = locate(Some(&elf), &mut memory, "device_config").unwrap() else {
            panic!("expected a node");
        };
        let text = render(&elf, &mut memory, &node).unwrap();
        assert!(text.contains("  id: "), "{text}");
        assert!(text.contains(" = 7\n"), "{text}");
        assert!(text.contains("    value: float = 2.5\n"), "{text}");
    }
}

#[cfg(test)]
mod rust_tests {
    use super::*;
    use crate::memory::Fake;
    use crate::path::{Location, locate};
    use studio_dwarf::ElfParser;

    fn initialized(path: &str) -> Fake {
        Fake::from_elf(path)
    }

    #[test]
    fn renders_rust_types() {
        let path = format!(
            "{}/../studio-dwarf/tests/fixtures/rust_v0.elf",
            env!("CARGO_MANIFEST_DIR")
        );
        let elf = ElfParser::parse(&path).unwrap();
        let mut memory = initialized(&path);
        let Location::Node(node) = locate(Some(&elf), &mut memory, "GIMBAL").unwrap() else {
            panic!("expected a node");
        };
        let text = render(&elf, &mut memory, &node).unwrap();
        for line in [
            "  mode: Mode = Idle (0)\n",
            "    kd: f32 = 0.1\n",
            "    limit: Option<f32> = Some\n      __0: f32 = 10.0\n",
            "    limit: Option<f32> = None\n  command",
            "    [3]: f32 = 0.0\n",
            "    __1: i16 = 0\n",
        ] {
            assert!(text.contains(line), "missing {line:?} in\n{text}");
        }
    }

    #[test]
    fn parses_rust_paths() {
        let path = format!(
            "{}/../studio-dwarf/tests/fixtures/rust_v0.elf",
            env!("CARGO_MANIFEST_DIR")
        );
        let elf = ElfParser::parse(&path).unwrap();
        let mut memory = initialized(&path);
        for (text, expected) in [
            (
                "GIMBAL.pitch.limit#Some.0",
                "rust_fixture::control::GIMBAL.pitch.limit#Some.__0",
            ),
            (
                "rust_fixture::control::GIMBAL.offset.1",
                "rust_fixture::control::GIMBAL.offset.__1",
            ),
        ] {
            let Location::Node(node) = locate(Some(&elf), &mut memory, text).unwrap() else {
                panic!("expected a node");
            };
            assert_eq!(node.path, expected);
        }
        let Location::Node(node) = locate(Some(&elf), &mut memory, "GIMBAL.pitch.kp").unwrap()
        else {
            panic!("expected a node");
        };
        assert_eq!(
            render(&elf, &mut memory, &node).unwrap(),
            format!("{}: f32 = 1.0\n", node.path)
        );
    }
}

#[cfg(test)]
mod lab_tests {
    use super::*;
    use crate::memory::Fake;
    use crate::path::{Location, locate};
    use studio_dwarf::ElfParser;

    const ELF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.elf");
    const EXPECTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.txt");

    /// Every lab static as the target holds it after startup, before `main` runs.
    /// Set `UPDATE_EXPECTED=1` to rewrite the expected output.
    #[test]
    fn renders_every_lab_static() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let mut names: Vec<_> = elf
            .variables()
            .into_iter()
            .map(|s| s.demangled_name.clone())
            .filter(|name| {
                [
                    "scalars",
                    "atomics",
                    "enums",
                    "aggregates",
                    "pointers",
                    "live",
                ]
                .iter()
                .any(|module| name.starts_with(&format!("type_lab::{module}::")))
            })
            .collect();
        names.sort();
        names.dedup();
        let mut text = String::new();
        for name in names {
            let Location::Node(node) = locate(Some(&elf), &mut memory, &name).unwrap() else {
                panic!("{name} has no type information");
            };
            text += &render(&elf, &mut memory, &node).unwrap();
        }
        if std::env::var_os("UPDATE_EXPECTED").is_some() {
            std::fs::write(EXPECTED, &text).unwrap();
        }
        assert_eq!(text, std::fs::read_to_string(EXPECTED).unwrap());
    }
}
