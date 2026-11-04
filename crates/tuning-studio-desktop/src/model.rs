use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct Value {
    pub path: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub kind: String,
    pub writable: bool,
    pub display: String,
    pub error: Option<String>,
    pub write_reason: Option<String>,
    pub label: Option<String>,
    #[serde(default)]
    pub children: Vec<Value>,
    pub children_note: Option<String>,
    #[serde(default)]
    pub expandable: bool,
}

#[derive(Deserialize)]
struct Inspection {
    version: u32,
    values: Vec<Value>,
}

pub fn parse_values(text: &str) -> Result<Vec<Value>> {
    let parsed: Inspection = serde_json::from_str(text)?;
    ensure!(
        parsed.version == 1,
        "Unsupported inspection version {}",
        parsed.version
    );
    fn validate(
        values: &mut [Value],
        parent_read_only: bool,
        count: &mut usize,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth <= 8, "Inspection exceeds depth limit");
        for value in values {
            *count += 1;
            ensure!(
                *count <= 10000 && !value.path.is_empty(),
                "Invalid inspection tree"
            );
            ensure!(
                ["raw", "watch", "watch_mut", "param"].contains(&value.kind.as_str()),
                "Unknown value kind"
            );
            let read_only = parent_read_only || value.kind == "watch" || value.error.is_some();
            if read_only {
                value.writable = false;
            }
            for child in &mut value.children {
                if let Some(error) = &value.error {
                    child.error = Some(error.clone());
                }
                ensure!(descendant(&child.path, &value.path), "Invalid child path");
            }
            validate(&mut value.children, read_only, count, depth + 1)?;
        }
        Ok(())
    }
    let mut values = parsed.values;
    validate(&mut values, false, &mut 0, 0)?;
    Ok(values)
}

pub fn descendant(path: &str, parent: &str) -> bool {
    path.strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with(['.', '[', '#']))
}

pub fn find<'a>(values: &'a [Value], path: &str) -> Option<&'a Value> {
    values.iter().find_map(|v| {
        if v.path == path {
            Some(v)
        } else {
            find(&v.children, path)
        }
    })
}

impl Value {
    pub fn raw(&self) -> String {
        let header = self.display.lines().next().unwrap_or(&self.display);
        let text = header
            .split_once(" = ")
            .map_or(self.display.as_str(), |(_, value)| value)
            .trim();
        if text.starts_with(['"', '\'']) {
            let quote = text.as_bytes()[0];
            let mut escaped = false;
            for (i, byte) in text.bytes().enumerate().skip(1) {
                if byte == quote && !escaped {
                    return text[..=i].to_owned();
                }
                escaped = byte == b'\\' && !escaped;
            }
        }
        let first = text.split_whitespace().next().unwrap_or(text);
        let scalar = first == "true"
            || first == "false"
            || first.parse::<f64>().is_ok()
            || first
                .strip_prefix("0x")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_hexdigit()));
        if scalar {
            first.to_owned()
        } else {
            text.to_owned()
        }
    }

    pub fn reason(&self) -> &str {
        self.error
            .as_deref()
            .or(self.write_reason.as_deref())
            .unwrap_or(match self.kind.as_str() {
                "watch" => {
                    "Read-only snapshot. Use watch_mut! at the task's update point for edits."
                }
                "watch_mut" => "Applied when firmware next calls watch_mut!.",
                "param" => "Firmware applies the declared range at get().",
                _ if self.writable => {
                    "Direct memory write. Task state can change or overwrite this value."
                }
                _ => "Select a scalar field, or expose it with watch_mut!.",
            })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Suggestion {
    pub path: String,
    pub label: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

pub fn parse_suggestions(text: &str) -> Result<Vec<Suggestion>> {
    #[derive(Deserialize)]
    struct Completions {
        version: u32,
        items: Vec<Suggestion>,
    }
    let parsed: Completions = serde_json::from_str(text)?;
    ensure!(
        parsed.version == 1 && parsed.items.len() <= 50,
        "Unsupported suggestions"
    );
    Ok(parsed.items)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub cli: String,
    pub chip: String,
    pub elf: String,
    pub probe: String,
    pub speed: u32,
    pub poll_ms: u64,
    pub watches: Vec<String>,
    pub expanded: BTreeSet<String>,
    pub locked: BTreeSet<String>,
    pub plot: crate::plot::Settings,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            cli: default_cli(),
            chip: "STM32H723VGTx".into(),
            elf: String::new(),
            probe: String::new(),
            speed: 4000,
            poll_ms: 500,
            watches: Vec::new(),
            expanded: BTreeSet::new(),
            locked: BTreeSet::new(),
            plot: crate::plot::Settings::default(),
        }
    }
}

fn default_cli() -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let sibling = parent.join("tuning-studio");
        if sibling.is_file() {
            return sibling.to_string_lossy().into_owned();
        }
    }
    "tuning-studio".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(display: &str) -> Value {
        serde_json::from_value(serde_json::json!({"path":"x", "type":"u64", "kind":"raw", "writable":true, "display":display})).unwrap()
    }
    #[test]
    fn copying_preserves_large_integer_digits_and_quoted_payloads() {
        assert_eq!(
            value("x: u64 = 18446744073709551615 ticks").raw(),
            "18446744073709551615"
        );
        assert_eq!(
            value("x: str = \"two words\" suffix").raw(),
            "\"two words\""
        );
        assert_eq!(value("gain: f32 = 0.25 (default 0.1)").raw(), "0.25");
    }
    #[test]
    fn read_only_snapshots_cannot_become_writable_through_children() {
        let values = parse_values(r#"{"version":1,"values":[{"path":"state","type":"State","kind":"watch","writable":false,"display":"state","children":[{"path":"state.kp","type":"f32","kind":"raw","writable":true,"display":"kp = 1"}]}]}"#).unwrap();
        assert!(!values[0].children[0].writable);
        assert!(parse_values(r#"{"version":2,"values":[]}"#).is_err());
    }
}
