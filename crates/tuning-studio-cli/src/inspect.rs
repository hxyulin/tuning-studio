//! Versioned JSON samples for editor clients, using the same snapshots as text reads.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use studio_dwarf::tree::{NodeKind, Step, SymbolNode};
use studio_dwarf::{ElfInfo, VariableType};

use crate::api::{self, Kind};
use crate::memory::Memory;
use crate::path::{self, Location};
use crate::value;

#[derive(Debug, Serialize)]
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
}

#[derive(Debug, Serialize)]
struct Inspection {
    version: u32,
    values: Vec<Sample>,
}

/// Discover API values or sample the requested paths, preserving per-location errors.
pub fn inspect(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    locations: &[String],
) -> Result<String> {
    memory.forget();
    let selected = if locations.is_empty() {
        let elf = elf.ok_or_else(|| anyhow!("the session has no ELF to discover values"))?;
        api::declared(elf, memory)
            .into_iter()
            .map(|(_, node)| (node.path.clone(), Ok(Location::Node(Box::new(node)))))
            .collect::<Vec<_>>()
    } else {
        locations
            .iter()
            .map(|text| (text.clone(), path::locate(elf, memory, text)))
            .collect()
    };
    let values = selected
        .into_iter()
        .map(|(text, location)| {
            let mut sample = Sample {
                path: text,
                type_name: String::new(),
                kind: "raw",
                writable: false,
                display: String::new(),
                write_reason: None,
                number: None,
                error: None,
            };
            let result = location.and_then(|location| sample.read(elf, memory, &location));
            if let Err(error) = result {
                sample.writable = false;
                sample.write_reason = Some(format!("{error:#}"));
                sample.error = Some(format!("{error:#}"));
                sample.number = None;
            }
            memory.forget();
            sample
        })
        .collect();
    Ok(serde_json::to_string(&Inspection { version: 1, values })? + "\n")
}

impl Sample {
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
                            self.write_reason = Some("watch! publishes a read-only snapshot; use watch_mut! with a mutable scalar at the task update point".into());
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
                    }
                }
            }
        }
        self.display = self.display.trim_end().to_owned();
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
}
