//! Rust wrapper presentation and debugger write policies. Layouts come from DWARF.

use anyhow::{Result, anyhow, bail};
use studio_dwarf::ElfInfo;
use studio_dwarf::tree::{NodeKind, NodeRef, Step, SymbolNode, WrapperKind};

use crate::{memory::Memory, path, value};

pub fn type_name(node: &SymbolNode) -> String {
    node.wrappers
        .iter()
        .find(|w| !matches!(w.kind, WrapperKind::UnsafeCell | WrapperKind::MaybeUninit))
        .or_else(|| {
            node.wrappers
                .iter()
                .find(|w| w.kind == WrapperKind::MaybeUninit)
        })
        .or_else(|| node.wrappers.first())
        .map_or_else(|| node.type_name.clone(), |w| w.name.clone())
}

fn has(node: &SymbolNode, kind: WrapperKind) -> bool {
    node.wrappers.iter().any(|w| w.kind == kind)
}

pub fn atomic(node: &SymbolNode) -> bool {
    has(node, WrapperKind::Atomic)
}

/// Inspect refcounts only when the user opens a shared pointer. The implicit
/// allocation weak owner is excluded from the displayed weak count.
pub fn shared_counts(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<Option<String>> {
    if !has(node, WrapperKind::Arc) && !has(node, WrapperKind::Rc) {
        return Ok(None);
    }
    let inner = node
        .pointee_type
        .ok_or_else(|| anyhow!("unsupported shared-pointer layout"))?;
    let width = if elf.is_64bit { 8 } else { 4 };
    let address = memory.word(node.address, width)?;
    if address == 0 {
        bail!("null pointer")
    }
    let header = NodeRef::typed(node.path.clone(), address, inner);
    let mut count = |name: &str| -> Result<u64> {
        let field = path::describe(elf, memory, &header.child(Step::Member(name.into())))?;
        let scalar = value::scalar_of(&field)
            .ok_or_else(|| anyhow!("unsupported reference-count layout"))?;
        let size = value::size_of(scalar);
        if !matches!(size, 4 | 8) {
            bail!("unsupported reference-count width")
        }
        memory.word(field.address, size)
    };
    let strong = count("strong")?;
    if strong == 0 {
        bail!("shared allocation has no strong owners")
    }
    let weak = count("weak")?;
    let weak_text = if weak
        == if width == 8 {
            u64::MAX
        } else {
            u64::from(u32::MAX)
        } {
        "weak locked".into()
    } else {
        format!("weak {}", weak.saturating_sub(1))
    };
    Ok(Some(format!("strong {strong} · {weak_text}")))
}

/// RefCell borrow state is presentation metadata, not a lock held by the host.
pub fn borrow_state(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<Option<u64>> {
    if !has(node, WrapperKind::RefCell) {
        return Ok(None);
    }
    let borrow = path::describe(elf, memory, &node.node.child(Step::Member("borrow".into())))?;
    Ok(Some(
        memory.word(borrow.address, if elf.is_64bit { 8 } else { 4 })?,
    ))
}

pub fn write_restriction(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<Option<String>> {
    if atomic(node) {
        let size = value::scalar_of(node).map(value::size_of).unwrap_or(0);
        if !matches!(size, 1 | 2 | 4) || !node.address.is_multiple_of(size as u64) {
            return Ok(Some(
                "atomic edits require an aligned 1-, 2-, or 4-byte scalar".into(),
            ));
        }
    }
    let parents = (0..=node.node.steps.len())
        .map(|end| {
            let reference = NodeRef {
                steps: node.node.steps[..end].to_vec(),
                ..node.node.clone()
            };
            path::describe(elf, memory, &reference)
        })
        .collect::<Result<Vec<_>>>()?;
    let interior = parents.iter().any(|parent| {
        has(parent, WrapperKind::Atomic)
            || has(parent, WrapperKind::Cell)
            || has(parent, WrapperKind::RefCell)
    });
    for (end, parent) in parents.iter().enumerate() {
        let step = node.node.steps.get(end);
        if (has(parent, WrapperKind::Arc)
            || has(parent, WrapperKind::Rc)
            || has(parent, WrapperKind::Weak))
            && matches!(step, Some(Step::Member(_)))
        {
            return Ok(Some(
                "shared-pointer bookkeeping is read-only; expand the pointer payload".into(),
            ));
        }
        if atomic(parent) && matches!(step, Some(Step::Member(_))) {
            return Ok(Some(
                "edit the atomic value itself, not its storage fields".into(),
            ));
        }
        if has(parent, WrapperKind::RefCell)
            && matches!(step, Some(Step::Member(name)) if name == "borrow")
        {
            return Ok(Some("RefCell borrow bookkeeping is read-only".into()));
        }
        if !interior
            && (has(parent, WrapperKind::Arc)
                || has(parent, WrapperKind::Rc)
                || has(parent, WrapperKind::Weak))
        {
            return Ok(Some(
                "shared ownership: edit through watch_mut! or an interior-mutable scalar".into(),
            ));
        }
        if borrow_state(elf, memory, parent)?.is_some_and(|state| state != 0) {
            return Ok(Some(
                "RefCell is borrowed; try again after the borrow ends".into(),
            ));
        }
        let follows = node
            .node
            .steps
            .get(end)
            .is_some_and(|step| matches!(step, Step::Deref | Step::SliceIndex(_)));
        let shared_ref =
            parent.type_name.starts_with('&') && !parent.type_name.starts_with("&mut ");
        let const_ptr = parent.type_name.starts_with("*const ");
        if follows && !interior && (shared_ref || const_ptr) {
            return Ok(Some(
                "shared reference or const pointer: publish an editable value with watch_mut!"
                    .into(),
            ));
        }
    }
    // An indirect path's original symbol can be writable while its target is in flash.
    let size = node.size.unwrap_or(1).max(1);
    if elf.variables().iter().any(|symbol| {
        symbol.is_readable()
            && !symbol.writable
            && symbol.size > 0
            && node.address >= symbol.address
            && node
                .address
                .checked_add(size)
                .is_some_and(|end| end <= symbol.address.saturating_add(symbol.size))
    }) {
        return Ok(Some("value is in a read-only section".into()));
    }
    Ok(None)
}

pub fn note(elf: &ElfInfo, memory: &mut dyn Memory, node: &SymbolNode) -> Result<Option<String>> {
    if let Some(state) = borrow_state(elf, memory, node)? {
        return Ok(Some(if state == 0 {
            "RefCell · unborrowed".into()
        } else {
            "RefCell · borrowed".into()
        }));
    }
    if has(node, WrapperKind::MaybeUninit) {
        return Ok(Some("MaybeUninit · initialization state is unknown".into()));
    }
    if atomic(node) {
        return Ok(Some(
            "Atomic storage · debugger access does not provide Rust memory ordering".into(),
        ));
    }
    if has(node, WrapperKind::Cell) || has(node, WrapperKind::UnsafeCell) {
        return Ok(Some("Interior-mutable storage · live debugger read".into()));
    }
    if node.kind == NodeKind::Pointer {
        return Ok(Some(
            "Pointee reads are live and may race firmware changes".into(),
        ));
    }
    Ok(None)
}
