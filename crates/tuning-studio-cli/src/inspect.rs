//! Versioned JSON samples for editor clients, using the same snapshots as text reads.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use studio_dwarf::tree::{self, NodeKind, Step, SymbolNode};
use studio_dwarf::{ElfInfo, VariableType};

use crate::api::{self, Kind};
use crate::memory::Memory;
use crate::path::{self, Location};
use crate::value;

#[derive(Debug, Clone, Serialize)]
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
    number: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Sample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    children_note: Option<String>,
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

/// Discover API values or sample the requested paths, preserving per-location errors.
pub fn inspect(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    locations: &[String],
) -> Result<String> {
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
                sample_location(Some(elf), memory, path, Ok(Location::Node(Box::new(node))))
            })
            .collect::<Vec<_>>()
    } else {
        locations
            .iter()
            .map(|text| {
                let location = path::locate(elf, memory, text);
                // An exact exported symbol wins over a logical field with the same name.
                if matches!(&location, Ok(Location::Node(node)) if node.node.steps.is_empty()) {
                    return sample_location(elf, memory, text.clone(), location);
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
                    );
                    if let Some(child) = sample.find(text) {
                        return child.clone();
                    }
                    if sample.error.is_some() {
                        sample.path.clone_from(text);
                        return sample;
                    }
                }
                sample_location(elf, memory, text.clone(), location)
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
) -> Sample {
    let mut sample = Sample {
        path: text,
        type_name: String::new(),
        kind: "raw",
        writable: false,
        display: String::new(),
        write_reason: None,
        number: None,
        error: None,
        label: None,
        children: Vec::new(),
        children_note: None,
    };
    if let Err(error) = location.and_then(|location| sample.read(elf, memory, &location)) {
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
    ) -> Result<()> {
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
                        let mut remaining = NODE_LIMIT;
                        self.expand(elf, memory, &value, 0, &mut remaining)?;
                    }
                    None => {
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
                        let mut remaining = NODE_LIMIT;
                        self.expand(elf, memory, node, 0, &mut remaining)?;
                    }
                }
            }
        }
        self.display = self.display.trim_end().to_owned();
        Ok(())
    }

    /// Descendants use the parent's snapshot. Never follow pointers or slices here:
    /// their pointees are outside the bytes validated by a watch's counters.
    fn expand(
        &mut self,
        elf: &ElfInfo,
        memory: &mut dyn Memory,
        node: &SymbolNode,
        depth: usize,
        remaining: &mut usize,
    ) -> Result<()> {
        if !node.expandable || node.kind == NodeKind::Pointer || node.sequence {
            return Ok(());
        }
        if depth == DEPTH_LIMIT || *remaining == 0 {
            self.children_note = Some("More fields omitted: inspection limit".into());
            return Ok(());
        }
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
        )
        .map_err(|e| anyhow!("{e}"))?;
        let shown = page.nodes.len();
        for child in page.nodes {
            if matches!(child.node.steps.last(), Some(Step::Discriminant)) {
                continue;
            }
            if let Some(Step::Variant(name)) = child.node.steps.last()
                && active.as_ref() != Some(name)
            {
                continue;
            }
            if *remaining == 0 {
                self.children_note = Some("More fields omitted: inspection limit".into());
                break;
            }
            *remaining -= 1;
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
                number: None,
                error: None,
                label: Some(child.label.clone()),
                children: Vec::new(),
                children_note: None,
            };
            let read = (|| -> Result<()> {
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
                sample.expand(elf, memory, &child, depth + 1, remaining)?;
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
        if page.total > shown as u64 {
            self.children_note = Some(format!("First {shown} of {} fields", page.total));
        }
        Ok(())
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
}
