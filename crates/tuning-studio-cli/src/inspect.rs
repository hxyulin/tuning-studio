//! Versioned JSON samples for editor clients, using the same snapshots as text reads.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use studio_dwarf::tree::{self, NodeKind, Step, SymbolNode};
use studio_dwarf::{ElfInfo, VariableType};

use crate::api::{self, Kind};
use crate::memory::Memory;
use crate::path::{self, Location};
use crate::{value, wrappers};

#[derive(Debug, Clone, Default, Serialize)]
struct Sample {
    path: String,
    #[serde(rename = "type")]
    type_name: String,
    kind: &'static str,
    writable: bool,
    display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    write_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    persistence_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    number: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Sample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    children_note: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    expandable: bool,
}

#[derive(Debug, Serialize)]
struct Inspection {
    version: u32,
    values: Vec<Sample>,
}

const CHILD_LIMIT: usize = 64;
const DEPTH_LIMIT: usize = 6;
const NODE_LIMIT: usize = 256;
const WATCH_REASON: &str = "watch! publishes a read-only snapshot; use watch_mut! with a mutable scalar at the task update point";

fn is_false(value: &bool) -> bool {
    !value
}

struct Expansion<'a> {
    paths: &'a [String],
    remaining: usize,
    bytes: u64,
    ancestors: Vec<(u64, String)>,
}

#[cfg(test)]
pub fn inspect(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    locations: &[String],
) -> Result<String> {
    inspect_expanded(elf, memory, locations, &[])
}

/// Discover API values or sample the requested paths, preserving per-location errors.
pub fn inspect_expanded(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    locations: &[String],
    expansions: &[String],
) -> Result<String> {
    if expansions.len() > 64 || expansions.iter().any(|path| path.len() > 1024) {
        bail!("at most 64 expansion paths of up to 1024 bytes are supported");
    }
    memory.forget();
    let declarations = elf
        .map(|elf| api::declared(elf, memory))
        .unwrap_or_default();
    let values = if locations.is_empty() {
        let elf = elf.ok_or_else(|| anyhow!("the session has no ELF to discover values"))?;
        declarations
            .into_iter()
            .map(|(_, node)| {
                let path = node.path.clone();
                sample_location(
                    Some(elf),
                    memory,
                    path,
                    Ok(Location::Node(Box::new(node))),
                    expansions,
                )
            })
            .collect::<Vec<_>>()
    } else {
        locations
            .iter()
            .map(|text| {
                let location = path::locate(elf, memory, text);
                // An exact exported symbol wins over a logical field with the same name.
                if matches!(&location, Ok(Location::Node(node)) if node.node.steps.is_empty()) {
                    return sample_location(elf, memory, text.clone(), location, expansions);
                }
                // Logical watch fields omit mailbox storage (e.g. motor.state.speed).
                // Read their owning snapshot once, rather than sampling its backing field.
                if let Some((_, root)) = declarations
                    .iter()
                    .filter(|(kind, root)| {
                        *kind == Kind::Watch
                            && text.strip_prefix(&root.path).is_some_and(|suffix| {
                                suffix.starts_with('.')
                                    || suffix.starts_with('[')
                                    || suffix.starts_with('#')
                            })
                    })
                    .max_by_key(|(_, root)| root.path.len())
                {
                    let mut sample = sample_location(
                        elf,
                        memory,
                        root.path.clone(),
                        Ok(Location::Node(Box::new(root.clone()))),
                        expansions,
                    );
                    if let Some(child) = sample.find(text) {
                        return child.clone();
                    }
                    if sample.error.is_some() {
                        sample.path.clone_from(text);
                        return sample;
                    }
                }
                sample_location(elf, memory, text.clone(), location, expansions)
            })
            .collect()
    };
    Ok(serde_json::to_string(&Inspection { version: 1, values })? + "\n")
}

fn sample_location(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    text: String,
    location: Result<Location>,
    expansions: &[String],
) -> Sample {
    let mut sample = Sample {
        path: text,
        type_name: String::new(),
        kind: "raw",
        writable: false,
        display: String::new(),
        write_reason: None,
        persistence_key: None,
        number: None,
        error: None,
        label: None,
        children: Vec::new(),
        children_note: None,
        expandable: false,
    };
    if let Err(error) =
        location.and_then(|location| sample.read(elf, memory, &location, expansions))
    {
        sample.writable = false;
        sample.write_reason = Some(format!("{error:#}"));
        sample.error = Some(format!("{error:#}"));
        sample.number = None;
        sample.children.clear();
    }
    memory.forget();
    sample
}

impl Sample {
    fn find(&self, path: &str) -> Option<&Self> {
        if self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(path))
    }

    fn read(
        &mut self,
        elf: Option<&ElfInfo>,
        memory: &mut dyn Memory,
        location: &Location,
        expansions: &[String],
    ) -> Result<()> {
        let mut expansion = Expansion {
            paths: expansions,
            remaining: NODE_LIMIT,
            bytes: 64 * 1024,
            ancestors: Vec::new(),
        };
        match location {
            Location::Raw { address, scalar } => {
                self.type_name = scalar.to_string();
                self.writable = true;
                let bytes = memory.read(*address, value::size_of(*scalar))?;
                let text = value::decode(*scalar, &bytes);
                self.display = format!("{address:#010x}: {scalar} = {text}");
                self.number = numeric(*scalar, &text);
            }
            Location::Node(node) => {
                let elf = elf.ok_or_else(|| anyhow!("symbol paths need an ELF"))?;
                self.type_name = node.type_name.clone();
                match api::kind(node) {
                    Some(kind) => {
                        self.kind = match kind {
                            Kind::Param => "param",
                            Kind::WatchMut => "watch_mut",
                            Kind::Watch => "watch",
                        };
                        self.writable = kind != Kind::Watch;
                        if kind == Kind::Param {
                            self.persistence_key = api::persistence_key(elf, memory, node);
                        }
                        if kind == Kind::Watch {
                            self.write_reason = Some(WATCH_REASON.into());
                        }
                        let value = api::traced_value(elf, memory, node)?.0;
                        self.type_name = value.type_name.clone();
                        self.display = match kind {
                            Kind::Param => api::render_param(elf, memory, node)?,
                            Kind::Watch | Kind::WatchMut => api::render_watch(elf, memory, node)?,
                        };
                        if kind != Kind::Param {
                            // render_watch leaves the validated snapshot in memory. Do not
                            // turn an unpublished or torn snapshot into a graph sample.
                            let start = api::field(elf, memory, node, "start")?;
                            let end = api::field(elf, memory, node, "end")?;
                            let starts = memory.word(start.address, 4)?;
                            if starts == 0 {
                                bail!("not written yet");
                            }
                            if starts != memory.word(end.address, 4)? {
                                bail!("value changed during each read");
                            }
                        }
                        self.number = node_number(memory, &value)?;
                        memory.retain_snapshots();
                        self.expand(elf, memory, &value, 0, &mut expansion)?;
                    }
                    None => {
                        self.type_name = wrappers::type_name(node);
                        self.children_note = wrappers::note(elf, memory, node)?;
                        self.write_reason = write_restriction(elf, memory, node)?;
                        self.writable = self.write_reason.is_none();
                        if !node.readable {
                            bail!("{}", node.status.as_deref().unwrap_or("not readable"));
                        }
                        if node.bit_size.is_some() {
                            bail!("bitfields are not supported yet");
                        }
                        self.display = value::render(elf, memory, node)?;
                        self.number = node_number(memory, node)?;
                        memory.retain_snapshots();
                        self.expand(elf, memory, node, 0, &mut expansion)?;
                    }
                }
            }
        }
        self.display = self.display.trim_end().to_owned();
        Ok(())
    }

    /// Inline descendants share a snapshot. Pointees are requested explicitly and
    /// read separately, with bounded bytes and a final pointer stability check.
    fn expand(
        &mut self,
        elf: &ElfInfo,
        memory: &mut dyn Memory,
        node: &SymbolNode,
        depth: usize,
        expansion: &mut Expansion<'_>,
    ) -> Result<()> {
        let indirect = node.kind == NodeKind::Pointer || node.sequence;
        self.expandable = indirect;
        if !node.expandable && !indirect {
            return Ok(());
        }
        if indirect && !expansion.paths.contains(&self.path) {
            return Ok(());
        }
        if depth == DEPTH_LIMIT || expansion.remaining == 0 {
            self.children_note = Some("More fields omitted: inspection limit".into());
            return Ok(());
        }
        let pointer_bytes = if indirect {
            let size = if node.kind == NodeKind::Pointer {
                if elf.is_64bit { 8 } else { 4 }
            } else {
                node.size.unwrap_or(0).min(16) as usize
            };
            let bytes = memory.read(node.address, size)?;
            match wrappers::shared_counts(elf, memory, node) {
                Ok(Some(note)) => self.add_note(&note),
                Ok(None) => {}
                Err(error) => {
                    self.children.push(self.failed_child(&format!("{error:#}")));
                    return Ok(());
                }
            }
            self.add_note("Pointee · live debugger read, outside the parent's snapshot");
            Some(bytes)
        } else {
            None
        };
        let active = if node.kind == NodeKind::TaggedEnum {
            tree::preview(elf, &node.node, &mut |address, len| {
                memory.read(address, len).map_err(|e| format!("{e:#}"))
            })
            .map_err(|e| anyhow!("{e}"))?
            .and_then(|p| p.active_variant)
        } else {
            None
        };
        let page = tree::children_page(
            elf,
            &node.node,
            0,
            CHILD_LIMIT,
            &mut |address, len| memory.word(address, len).map_err(|e| format!("{e:#}")),
            true,
        );
        let page = match page {
            Ok(page) => page,
            Err(error) if indirect => {
                self.children.push(self.failed_child(&error.to_string()));
                return Ok(());
            }
            Err(error) => return Err(anyhow!("{error}")),
        };
        let shown = page.nodes.len();
        expansion
            .ancestors
            .push((node.address, node.type_name.clone()));
        for child in page.nodes {
            if node
                .wrappers
                .iter()
                .any(|w| w.kind == tree::WrapperKind::RefCell)
                && child.label != "value"
            {
                continue;
            }
            if matches!(child.node.steps.last(), Some(Step::Discriminant)) {
                continue;
            }
            if let Some(Step::Variant(name)) = child.node.steps.last()
                && active.as_ref() != Some(name)
            {
                continue;
            }
            if expansion.remaining == 0 {
                self.children_note = Some("More fields omitted: inspection limit".into());
                break;
            }
            expansion.remaining -= 1;
            let suffix = child
                .path
                .strip_prefix(&node.path)
                .ok_or_else(|| anyhow!("child path is outside its parent"))?;
            let path = format!("{}{suffix}", self.path);
            let mut sample = Sample {
                path,
                type_name: child.type_name.clone(),
                kind: self.kind,
                writable: false,
                display: String::new(),
                write_reason: None,
                persistence_key: None,
                number: None,
                error: None,
                label: Some(child.label.clone()),
                children: Vec::new(),
                children_note: None,
                expandable: child.kind == NodeKind::Pointer || child.sequence,
            };
            let read = (|| -> Result<()> {
                let child = if node.kind == NodeKind::Pointer {
                    path::describe(elf, memory, &child.node)?
                } else {
                    child
                };
                sample.type_name = wrappers::type_name(&child);
                if indirect {
                    if expansion
                        .ancestors
                        .contains(&(child.address, child.type_name.clone()))
                    {
                        sample.expandable = false;
                        sample.children_note = Some("Cycle · already shown in an ancestor".into());
                        sample.error.clone_from(&sample.children_note);
                        sample.write_reason.clone_from(&sample.children_note);
                        sample.display = format!("{}: {}", sample.path, sample.type_name);
                        return Ok(());
                    }
                    let size = child
                        .size
                        .ok_or_else(|| anyhow!("pointee size is unknown"))?;
                    if size > expansion.bytes {
                        bail!(
                            "pointee exceeds the 64 KiB inspection budget; watch a specific field"
                        )
                    }
                    expansion.bytes -= size;
                    if size > 0 {
                        memory.snapshot(child.address, size as usize)?;
                    }
                }
                sample.children_note = wrappers::note(elf, memory, &child)?;
                if !child.readable {
                    bail!("{}", child.status.as_deref().unwrap_or("not readable"));
                }
                if child.bit_size.is_some() {
                    bail!("bitfields are not supported yet");
                }
                sample.write_reason = if self.kind == "watch" {
                    Some(WATCH_REASON.into())
                } else {
                    write_restriction(elf, memory, &child)?
                };
                sample.writable = sample.write_reason.is_none();
                sample.display = if child.sequence {
                    format!("{}: {}", sample.path, sample.type_name)
                } else {
                    value::render_snapshot(elf, memory, &child, &sample.path)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                };
                sample.number = node_number(memory, &child)?;
                sample.expand(elf, memory, &child, depth + 1, expansion)?;
                Ok(())
            })();
            if let Err(error) = read {
                sample.writable = false;
                sample.number = None;
                sample.error = Some(format!("{error:#}"));
                sample.write_reason.clone_from(&sample.error);
                sample.children.clear();
            }
            self.children.push(sample);
        }
        expansion.ancestors.pop();
        if let Some(bytes) = pointer_bytes
            && memory.transfer(node.address, bytes.len())? != bytes
        {
            self.children = vec![self.failed_child("Pointer changed during read")];
        }
        if page.total > shown as u64 {
            self.add_note(&format!("First {shown} of {} fields", page.total));
        }
        Ok(())
    }

    fn add_note(&mut self, text: &str) {
        match &mut self.children_note {
            Some(note) => {
                note.push('\n');
                note.push_str(text);
            }
            None => self.children_note = Some(text.into()),
        }
    }

    fn failed_child(&self, text: &str) -> Self {
        Self {
            path: format!("{}.*", self.path),
            label: Some("*".into()),
            kind: self.kind,
            error: Some(text.into()),
            write_reason: Some(text.into()),
            ..Self::default()
        }
    }
}

fn node_number(memory: &mut dyn Memory, node: &SymbolNode) -> Result<Option<f64>> {
    if node.kind != NodeKind::Scalar || node.type_name == "char" {
        return Ok(None);
    }
    let Some(scalar) = value::scalar_of(node) else {
        return Ok(None);
    };
    let bytes = memory.read(node.address, value::size_of(scalar))?;
    Ok(numeric(scalar, &value::decode(scalar, &bytes)))
}

fn numeric(scalar: VariableType, text: &str) -> Option<f64> {
    if matches!(scalar, VariableType::Bool | VariableType::Raw(_)) {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn write_restriction(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<Option<String>> {
    let reason = if !node.readable {
        Some(
            node.status
                .as_deref()
                .unwrap_or("value is unavailable in the current task state")
                .to_owned(),
        )
    } else if node.bit_size.is_some() {
        Some("bitfield writes are not supported".into())
    } else if !matches!(node.kind, NodeKind::Scalar | NodeKind::Enum)
        || value::scalar_of(node).is_none()
    {
        Some("select a supported scalar field; aggregate writes are not supported".into())
    } else if api::owner(elf, memory, node)?.is_some() {
        Some("API backing storage cannot be edited directly; edit the declared param or watch_mut! value".into())
    } else {
        None
    };
    if reason.is_some() {
        return Ok(reason);
    }
    if let Some(reason) = wrappers::write_restriction(elf, memory, node)? {
        return Ok(Some(reason));
    }
    let indirect = node
        .node
        .steps
        .iter()
        .any(|s| matches!(s, Step::Deref | Step::SliceIndex(_)));
    if !indirect
        && elf
            .find_symbol(&node.node.symbol)
            .is_some_and(|symbol| !symbol.writable)
    {
        return Ok(Some("value is in a read-only section".into()));
    }
    if node.node.steps.iter().any(|s| matches!(s, Step::Task(_))) {
        let size = value::size_of(value::scalar_of(node).ok_or_else(|| anyhow!("not a scalar"))?);
        if !matches!(size, 1 | 2 | 4) {
            return Ok(Some("task-local writes require a 1-, 2-, or 4-byte scalar; use watch_mut! with a supported scalar".into()));
        }
        if !node.address.is_multiple_of(size as u64) {
            return Ok(Some(
                "task-local address is not aligned for a single memory access".into(),
            ));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Fake;
    use studio_dwarf::ElfParser;

    const ELF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tune_lab.elf");

    #[test]
    fn discovers_capabilities_and_never_plots_unpublished_snapshots() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let result: serde_json::Value =
            serde_json::from_str(&inspect(Some(&elf), &mut memory, &[]).unwrap()).unwrap();
        assert_eq!(result["version"], 1);
        let values = result["values"].as_array().unwrap();
        let kp = values.iter().find(|v| v["path"] == "tune_lab::KP").unwrap();
        assert_eq!(kp["kind"], "param");
        assert_eq!(kp["writable"], true);
        assert!((kp["number"].as_f64().unwrap() - 0.02).abs() < 1e-7);
        for name in ["motor.speed", "motor.integral"] {
            let sample = values.iter().find(|v| v["path"] == name).unwrap();
            assert_eq!(sample["writable"], false);
            assert_eq!(sample["write_reason"], "not written yet");
            assert_eq!(sample["error"], "not written yet");
            assert!(sample.get("number").is_none());
        }
    }

    #[test]
    fn bad_paths_do_not_hide_good_values_and_backing_fields_are_read_only() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let paths = ["missing", "tune_lab::KP", "motor.integral.snapshot.value"].map(String::from);
        let result: serde_json::Value =
            serde_json::from_str(&inspect(Some(&elf), &mut memory, &paths).unwrap()).unwrap();
        let values = result["values"].as_array().unwrap();
        assert!(values[0]["error"].as_str().unwrap().contains("no symbol"));
        assert!(values[1].get("number").is_some());
        assert_eq!(values[2]["writable"], false);
        assert!(
            values[2]["write_reason"]
                .as_str()
                .unwrap()
                .contains("backing storage")
        );
        assert_eq!(values[0]["write_reason"], values[0]["error"]);
    }

    #[test]
    fn raw_nan_is_displayed_but_not_graphed() {
        let mut memory = Fake::new(0x2000_0000, f32::NAN.to_le_bytes().to_vec());
        let result: serde_json::Value =
            serde_json::from_str(&inspect(None, &mut memory, &["0x20000000:f32".into()]).unwrap())
                .unwrap();
        assert!(
            result["values"][0]["display"]
                .as_str()
                .unwrap()
                .contains("NaN")
        );
        assert!(result["values"][0].get("number").is_none());
    }

    #[test]
    fn numbers_come_from_validated_watch_snapshots() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let Location::Node(node) = path::locate(Some(&elf), &mut memory, "motor.integral").unwrap()
        else {
            panic!("expected a node");
        };
        for (name, bytes) in [
            ("end", 7u32.to_le_bytes()),
            ("value", 12.5f32.to_le_bytes()),
            ("start", 7u32.to_le_bytes()),
        ] {
            let field = api::field(&elf, &mut memory, &node, name).unwrap();
            memory.write(field.address, &bytes).unwrap();
        }
        let paths = ["motor.integral".into()];
        let result: serde_json::Value =
            serde_json::from_str(&inspect(Some(&elf), &mut memory, &paths).unwrap()).unwrap();
        assert_eq!(result["values"][0]["number"], 12.5);
        assert!(result["values"][0].get("error").is_none());
        let start = api::field(&elf, &mut memory, &node, "start").unwrap();
        memory.write(start.address, &8u32.to_le_bytes()).unwrap();
        let result: serde_json::Value =
            serde_json::from_str(&inspect(Some(&elf), &mut memory, &paths).unwrap()).unwrap();
        assert!(result["values"][0].get("number").is_none());
        assert!(
            result["values"][0]["error"]
                .as_str()
                .unwrap()
                .contains("changed")
        );
    }

    fn snapshot(elf: &ElfInfo, memory: &mut Fake) -> SymbolNode {
        let Location::Node(node) = path::locate(Some(elf), memory, "motor.state").unwrap() else {
            panic!("expected a node");
        };
        for name in ["end", "start"] {
            let counter = api::field(elf, memory, &node, name).unwrap();
            memory.write(counter.address, &1u32.to_le_bytes()).unwrap();
        }
        *node
    }

    #[test]
    fn nested_watch_fields_share_the_validated_snapshot_and_stay_read_only() {
        struct Changing {
            memory: Fake,
            trigger: u64,
            speed: u64,
            changed: bool,
        }
        impl Memory for Changing {
            fn transfer(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
                let bytes = self.memory.transfer(address, len)?;
                if address == self.trigger && !self.changed {
                    self.memory.write(self.speed, &99f32.to_le_bytes())?;
                    self.changed = true;
                }
                Ok(bytes)
            }
            fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
                self.memory.write(address, data)
            }
            fn snapshots(&mut self) -> &mut crate::memory::Snapshots {
                self.memory.snapshots()
            }
            fn ram(&self) -> &[std::ops::Range<u64>] {
                self.memory.ram()
            }
        }
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let root = snapshot(&elf, &mut memory);
        let value = api::field(&elf, &mut memory, &root, "value").unwrap();
        let speed = path::describe(
            &elf,
            &mut memory,
            &value.node.child(Step::Member("speed".into())),
        )
        .unwrap();
        memory.write(speed.address, &7f32.to_le_bytes()).unwrap();
        let end = api::field(&elf, &mut memory, &root, "end").unwrap();
        let mut memory = Changing {
            memory,
            trigger: end.address,
            speed: speed.address,
            changed: false,
        };
        let sample = sample_location(
            Some(&elf),
            &mut memory,
            root.path.clone(),
            Ok(Location::Node(Box::new(root))),
            &[],
        );
        assert!(memory.changed);
        assert_eq!(sample.find("motor.state.speed").unwrap().number, Some(7.0));
        let integral = sample.find("motor.state.pi.integral").unwrap();
        assert_eq!(integral.label.as_deref(), Some("integral"));
        assert_eq!(integral.number, Some(0.0));
        assert_eq!(integral.kind, "watch");
        assert!(!integral.writable);
        assert!(
            integral
                .write_reason
                .as_deref()
                .unwrap()
                .contains("read-only snapshot")
        );
        assert!(sample.find("motor.state.driver").is_none());
        assert_eq!(
            memory.memory.word(speed.address, 4).unwrap(),
            u64::from(99f32.to_bits())
        );
    }

    #[test]
    fn logical_watch_field_requests_validate_the_parent_counters() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let root = snapshot(&elf, &mut memory);
        let paths = ["motor.state.pi.integral".into()];
        let sample: serde_json::Value =
            serde_json::from_str(&inspect(Some(&elf), &mut memory, &paths).unwrap()).unwrap();
        assert_eq!(sample["values"][0]["path"], "motor.state.pi.integral");
        assert_eq!(sample["values"][0]["number"], 0.0);
        assert_eq!(sample["values"][0]["writable"], false);
        let counter = api::field(&elf, &mut memory, &root, "start").unwrap();
        memory.write(counter.address, &2u32.to_le_bytes()).unwrap();
        let sample: serde_json::Value =
            serde_json::from_str(&inspect(Some(&elf), &mut memory, &paths).unwrap()).unwrap();
        assert!(sample["values"][0].get("number").is_none());
        assert!(sample["values"][0].get("children").is_none());
        assert!(
            sample["values"][0]["error"]
                .as_str()
                .unwrap()
                .contains("changed")
        );
    }

    #[test]
    fn raw_structs_arrays_and_active_variants_have_typed_children() {
        const TYPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.elf");
        let elf = ElfParser::parse(TYPES).unwrap();
        let mut memory = Fake::from_elf(TYPES);
        let paths = [
            "type_lab::aggregates::ROBOT",
            "type_lab::aggregates::LARGE",
            "type_lab::aggregates::TABLE",
            "type_lab::enums::COMMAND",
        ]
        .map(String::from);
        let text = inspect(Some(&elf), &mut memory, &paths).unwrap();
        let result: serde_json::Value = serde_json::from_str(&text).unwrap();
        let robot = &result["values"][0];
        assert!(robot.get("error").is_none(), "{text}");
        let axes = robot["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == "axes")
            .unwrap();
        let position = axes["children"][0]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == "position")
            .unwrap();
        assert_eq!(position["number"], 0.5);
        assert_eq!(position["writable"], true);
        assert_eq!(
            position["path"],
            "type_lab::aggregates::ROBOT.axes[0].position"
        );
        assert_eq!(
            result["values"][1]["children"].as_array().unwrap().len(),
            CHILD_LIMIT
        );
        assert_eq!(
            result["values"][1]["children_note"],
            "First 64 of 300 fields"
        );
        assert_eq!(result["values"][2]["children"][0]["writable"], false);
        let variants = result["values"][3]["children"].as_array().unwrap();
        assert_eq!(
            variants.len(),
            1,
            "only the active payload is exposed: {text}"
        );
        assert_eq!(variants[0]["label"], "Position");
    }

    #[test]
    fn references_and_pointers_need_explicit_dereferencing() {
        const TYPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.elf");
        let elf = ElfParser::parse(TYPES).unwrap();
        let mut memory = Fake::from_elf(TYPES);
        let paths = [
            "type_lab::pointers::TO_U32",
            "type_lab::pointers::TO_U32.*",
            "type_lab::pointers::REFERENCE.*",
            "type_lab::pointers::TO_ARRAY.*",
            "type_lab::pointers::NULL.*",
        ]
        .map(String::from);
        let text = inspect(Some(&elf), &mut memory, &paths).unwrap();
        let result: serde_json::Value = serde_json::from_str(&text).unwrap();
        let values = result["values"].as_array().unwrap();
        assert!(
            values[0].get("children").is_none(),
            "pointer rows do not auto-follow"
        );
        assert!(
            values[0].get("number").is_none(),
            "pointer addresses are not plot samples"
        );
        assert_eq!(
            values[1]["number"].as_f64(),
            Some(f64::from(0xdead_beefu32))
        );
        assert_eq!(values[2]["number"].as_f64(), Some(3.0));
        assert_eq!(
            values[2]["kind"], "raw",
            "dereferencing remains a debugger memory operation"
        );
        assert_eq!(values[3]["children"].as_array().unwrap().len(), CHILD_LIMIT);
        assert!(
            values[4]["error"]
                .as_str()
                .unwrap()
                .contains("null pointer")
        );
    }

    const WRAPPERS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wrappers.elf");

    fn wrapper_node(elf: &ElfInfo, memory: &mut dyn Memory, name: &str) -> SymbolNode {
        let Location::Node(node) =
            path::locate(Some(elf), memory, &format!("wrapper_fixture::{name}")).unwrap()
        else {
            panic!("expected node")
        };
        *node
    }

    fn shared_allocation(elf: &ElfInfo, memory: &mut Fake, name: &str, offset: u64) -> SymbolNode {
        let root = wrapper_node(elf, memory, name);
        let heap = wrapper_node(elf, memory, "HEAP");
        let address = heap.address + offset;
        memory
            .write(root.address, &(address as u32).to_le_bytes())
            .unwrap();
        let header = tree::NodeRef::typed("allocation", address, root.pointee_type.unwrap());
        for (name, count) in [("strong", 2u32), ("weak", 2u32)] {
            let counter =
                path::describe(elf, memory, &header.child(Step::Member(name.into()))).unwrap();
            memory.write(counter.address, &count.to_le_bytes()).unwrap();
        }
        let mut payload = ["data", "value"]
            .into_iter()
            .find_map(|name| {
                path::describe(elf, memory, &header.child(Step::Member(name.into()))).ok()
            })
            .unwrap();
        if payload
            .wrappers
            .iter()
            .any(|w| w.kind == tree::WrapperKind::RefCell)
        {
            payload = path::describe(
                elf,
                memory,
                &payload.node.child(Step::Member("value".into())),
            )
            .unwrap();
        }
        memory.write(payload.address, &7u32.to_le_bytes()).unwrap();
        if payload.size.unwrap_or(0) > 4 {
            memory
                .write(payload.address + 4, &0.5f32.to_le_bytes())
                .unwrap();
        }
        root
    }

    fn expanded(
        elf: &ElfInfo,
        memory: &mut dyn Memory,
        name: &str,
        paths: &[&str],
    ) -> serde_json::Value {
        let root = format!("wrapper_fixture::{name}");
        let paths = paths
            .iter()
            .map(|p| format!("wrapper_fixture::{p}"))
            .collect::<Vec<_>>();
        serde_json::from_str(&inspect_expanded(Some(elf), memory, &[root], &paths).unwrap())
            .unwrap()
    }

    #[test]
    fn atomic_and_cell_values_keep_their_types_and_write_policies() {
        let elf = ElfParser::parse(WRAPPERS).unwrap();
        let mut memory = Fake::from_elf(WRAPPERS);
        for (name, number, expected) in
            [("ATOMIC", 42.0, "Atomic<u32>"), ("CELL", 9.0, "Cell<u32>")]
        {
            let sample = expanded(&elf, &mut memory, name, &[]);
            assert_eq!(sample["values"][0]["type"], expected);
            assert_eq!(sample["values"][0]["number"].as_f64(), Some(number));
            assert_eq!(sample["values"][0]["writable"], true);
        }
        let flag = expanded(&elf, &mut memory, "FLAG", &[]);
        assert!(
            flag["values"][0]["display"]
                .as_str()
                .unwrap()
                .contains("true")
        );
        assert!(
            crate::set_and_read(Some(&elf), &mut memory, "wrapper_fixture::FLAG", "2").is_err()
        );
        crate::set_and_read(Some(&elf), &mut memory, "wrapper_fixture::ATOMIC", "50").unwrap();
        crate::set_and_read(Some(&elf), &mut memory, "wrapper_fixture::CELL", "11").unwrap();
        assert_eq!(
            expanded(&elf, &mut memory, "ATOMIC", &[])["values"][0]["number"],
            50.0
        );
        assert_eq!(
            expanded(&elf, &mut memory, "CELL", &[])["values"][0]["number"],
            11.0
        );
    }

    #[test]
    fn box_pin_nonnull_and_references_expand_only_when_requested() {
        let elf = ElfParser::parse(WRAPPERS).unwrap();
        let mut memory = Fake::from_elf(WRAPPERS);
        let data = wrapper_node(&elf, &mut memory, "DATA");
        for name in ["BOXED", "PINNED", "NONNULL", "REFERENCE", "MUT_REFERENCE"] {
            let root = wrapper_node(&elf, &mut memory, name);
            memory
                .write(root.address, &(data.address as u32).to_le_bytes())
                .unwrap();
            let collapsed = expanded(&elf, &mut memory, name, &[]);
            assert_eq!(collapsed["values"][0]["expandable"], true);
            assert!(collapsed["values"][0].get("children").is_none());
            let shown = expanded(&elf, &mut memory, name, &[name]);
            assert_eq!(shown["values"][0]["children"][0]["label"], "*");
            let fields = shown["values"][0]["children"][0]["children"]
                .as_array()
                .unwrap();
            let count = fields.iter().find(|f| f["label"] == "count").unwrap();
            assert_eq!(count["number"], 7.0, "{name}: {shown}");
            assert_eq!(
                count["writable"],
                matches!(name, "BOXED" | "PINNED" | "MUT_REFERENCE"),
                "{name}: {shown}"
            );
        }
        crate::set_and_read(
            Some(&elf),
            &mut memory,
            "wrapper_fixture::BOXED.*.gain",
            "0.75",
        )
        .unwrap();
        assert!(
            crate::set_and_read(
                Some(&elf),
                &mut memory,
                "wrapper_fixture::REFERENCE.*.gain",
                "0.2"
            )
            .is_err()
        );
        // A collapsed invalid pointer never reads its pointee.
        let root = wrapper_node(&elf, &mut memory, "BOXED");
        memory
            .write(root.address, &0xFFFF_F000u32.to_le_bytes())
            .unwrap();
        assert!(
            expanded(&elf, &mut memory, "BOXED", &[])["values"][0]
                .get("error")
                .is_none()
        );
        assert!(
            expanded(&elf, &mut memory, "BOXED", &["BOXED"])["values"][0]["children"][0]["error"]
                .is_string()
        );
        memory.write(root.address, &0u32.to_le_bytes()).unwrap();
        assert!(
            expanded(&elf, &mut memory, "BOXED", &["BOXED"])["values"][0]["children"][0]["error"]
                .as_str()
                .unwrap()
                .contains("null")
        );
    }

    #[test]
    fn arc_and_rc_show_payloads_and_counts_without_editable_bookkeeping() {
        let elf = ElfParser::parse(WRAPPERS).unwrap();
        let mut memory = Fake::from_elf(WRAPPERS);
        for (name, offset) in [("ARC", 0), ("RC", 64)] {
            shared_allocation(&elf, &mut memory, name, offset);
            let shown = expanded(&elf, &mut memory, name, &[name]);
            assert!(
                shown["values"][0]["children_note"]
                    .as_str()
                    .unwrap()
                    .contains("strong 2 · weak 1")
            );
            let payload = &shown["values"][0]["children"][0];
            let fields = payload["children"].as_array().unwrap();
            assert!(
                fields
                    .iter()
                    .all(|f| f["label"] != "strong" && f["label"] != "weak")
            );
            let count = fields.iter().find(|f| f["label"] == "count").unwrap();
            assert_eq!(count["number"], 7.0, "{shown}");
            assert_eq!(count["writable"], false);
            assert!(
                crate::set_and_read(
                    Some(&elf),
                    &mut memory,
                    &format!("wrapper_fixture::{name}.*.count"),
                    "4"
                )
                .is_err()
            );
        }
        let raw_count = "wrapper_fixture::ARC.ptr.pointer.*.strong";
        assert!(path::locate(Some(&elf), &mut memory, raw_count).is_ok());
        assert!(
            crate::set_and_read(Some(&elf), &mut memory, raw_count, "4")
                .unwrap_err()
                .to_string()
                .contains("bookkeeping")
        );
        shared_allocation(&elf, &mut memory, "ARC_ATOMIC", 128);
        let shown = expanded(&elf, &mut memory, "ARC_ATOMIC", &["ARC_ATOMIC"]);
        assert_eq!(shown["values"][0]["children"][0]["number"], 7.0, "{shown}");
        assert_eq!(shown["values"][0]["children"][0]["writable"], true);
        crate::set_and_read(
            Some(&elf),
            &mut memory,
            "wrapper_fixture::ARC_ATOMIC.*",
            "9",
        )
        .unwrap();
        assert_eq!(
            expanded(&elf, &mut memory, "ARC_ATOMIC", &["ARC_ATOMIC"])["values"][0]["children"][0]
                ["number"],
            9.0
        );
        shared_allocation(&elf, &mut memory, "RC_REFCELL", 192);
        crate::set_and_read(
            Some(&elf),
            &mut memory,
            "wrapper_fixture::RC_REFCELL.*.value.count",
            "12",
        )
        .unwrap();
        let borrow = wrapper_node(&elf, &mut memory, "RC_REFCELL.*.borrow");
        memory.write(borrow.address, &1u32.to_le_bytes()).unwrap();
        assert!(
            crate::set_and_read(
                Some(&elf),
                &mut memory,
                "wrapper_fixture::RC_REFCELL.*.value.count",
                "13"
            )
            .is_err()
        );
        let weak = wrapper_node(&elf, &mut memory, "WEAK");
        let heap = wrapper_node(&elf, &mut memory, "HEAP");
        memory
            .write(weak.address, &(heap.address as u32).to_le_bytes())
            .unwrap();
        let shown = expanded(&elf, &mut memory, "WEAK", &["WEAK"]);
        assert!(
            shown["values"][0]["children"][0]["error"]
                .as_str()
                .unwrap()
                .contains("weak pointer")
        );
    }

    #[test]
    fn refcell_hides_borrow_storage_and_blocks_edits_during_a_borrow() {
        let elf = ElfParser::parse(WRAPPERS).unwrap();
        let mut memory = Fake::from_elf(WRAPPERS);
        let shown = expanded(&elf, &mut memory, "REFCELL", &[]);
        assert_eq!(shown["values"][0]["children"].as_array().unwrap().len(), 1);
        assert_eq!(shown["values"][0]["children"][0]["label"], "value");
        crate::set_and_read(
            Some(&elf),
            &mut memory,
            "wrapper_fixture::REFCELL.value.gain",
            "0.8",
        )
        .unwrap();
        assert!(
            crate::set_and_read(
                Some(&elf),
                &mut memory,
                "wrapper_fixture::REFCELL.borrow",
                "1"
            )
            .is_err()
        );
        let borrow = wrapper_node(&elf, &mut memory, "REFCELL.borrow");
        memory.write(borrow.address, &1u32.to_le_bytes()).unwrap();
        let shown = expanded(&elf, &mut memory, "REFCELL", &[]);
        assert!(
            shown["values"][0]["children_note"]
                .as_str()
                .unwrap()
                .contains("borrowed")
        );
        assert!(
            shown["values"][0]["children"][0]["children"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["writable"] == false)
        );
        assert!(
            crate::set_and_read(
                Some(&elf),
                &mut memory,
                "wrapper_fixture::REFCELL.value.gain",
                "0.1"
            )
            .is_err()
        );
    }

    #[test]
    fn pointer_cycles_stop_at_the_existing_ancestor() {
        let elf = ElfParser::parse(WRAPPERS).unwrap();
        let mut memory = Fake::from_elf(WRAPPERS);
        let root = wrapper_node(&elf, &mut memory, "CYCLE");
        let next = wrapper_node(&elf, &mut memory, "CYCLE.next");
        memory
            .write(next.address, &(root.address as u32).to_le_bytes())
            .unwrap();
        let shown = expanded(&elf, &mut memory, "CYCLE", &["CYCLE.next"]);
        let next = shown["values"][0]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["label"] == "next")
            .unwrap();
        assert!(
            next["children"][0]["children_note"]
                .as_str()
                .unwrap()
                .contains("Cycle")
        );
        assert!(next["children"][0].get("children").is_none());
        assert!(next["children"][0].get("expandable").is_none());
    }
    #[test]
    fn a_changed_pointer_discards_every_pointee_sample() {
        struct Flip {
            memory: Fake,
            trigger: u64,
            slot: u64,
            replacement: u32,
            flipped: bool,
        }
        impl Memory for Flip {
            fn transfer(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
                let bytes = self.memory.transfer(address, len)?;
                if address == self.trigger && !self.flipped {
                    self.memory
                        .write(self.slot, &self.replacement.to_le_bytes())?;
                    self.flipped = true;
                }
                Ok(bytes)
            }
            fn write(&mut self, address: u64, bytes: &[u8]) -> Result<()> {
                self.memory.write(address, bytes)
            }
            fn snapshots(&mut self) -> &mut crate::memory::Snapshots {
                self.memory.snapshots()
            }
            fn ram(&self) -> &[std::ops::Range<u64>] {
                self.memory.ram()
            }
        }
        let elf = ElfParser::parse(WRAPPERS).unwrap();
        let mut memory = Fake::from_elf(WRAPPERS);
        let data = wrapper_node(&elf, &mut memory, "DATA");
        let boxed = wrapper_node(&elf, &mut memory, "BOXED");
        let heap = wrapper_node(&elf, &mut memory, "HEAP");
        memory
            .write(boxed.address, &(data.address as u32).to_le_bytes())
            .unwrap();
        let mut memory = Flip {
            memory,
            trigger: data.address,
            slot: boxed.address,
            replacement: heap.address as u32,
            flipped: false,
        };
        let shown = expanded(&elf, &mut memory, "BOXED", &["BOXED"]);
        assert!(memory.flipped);
        let child = &shown["values"][0]["children"][0];
        assert_eq!(child["error"], "Pointer changed during read");
        assert!(child.get("number").is_none());
        assert!(child.get("children").is_none());
    }
}
