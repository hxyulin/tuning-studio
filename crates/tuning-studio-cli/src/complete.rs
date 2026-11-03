//! Bounded symbol and field suggestions for an editor's watch input.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use studio_dwarf::tree::{self, NodeKind, Step, SymbolNode};
use studio_dwarf::{ElfInfo, tasks};

use crate::{
    api,
    memory::{Memory, Snapshots},
    path, wrappers,
};

const LIMIT: usize = 50;

#[derive(Serialize)]
struct Item {
    path: String,
    label: String,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Serialize)]
struct Completions {
    version: u32,
    items: Vec<Item>,
}

/// Suggest roots or one level of fields. Reads memory only to resolve live paths.
pub fn complete(elf: &ElfInfo, memory: &mut dyn Memory, query: &str) -> Result<String> {
    complete_inner(elf, memory, query, true)
}

/// Complete static paths without attaching to a target. Live pointees are unavailable.
pub fn complete_metadata(elf: &ElfInfo, query: &str) -> Result<String> {
    complete_inner(elf, &mut NoTarget::default(), query, false)
}

#[derive(Default)]
struct NoTarget {
    snapshots: Snapshots,
}
impl Memory for NoTarget {
    fn transfer(&mut self, _: u64, _: usize) -> Result<Vec<u8>> {
        bail!("a running session is needed for this path")
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Result<()> {
        bail!("completion never writes memory")
    }
    fn snapshots(&mut self) -> &mut Snapshots {
        &mut self.snapshots
    }
    fn ram(&self) -> &[std::ops::Range<u64>] {
        &[]
    }
}

fn complete_inner(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    query: &str,
    live: bool,
) -> Result<String> {
    if query.len() > 1024 {
        bail!("completion query exceeds 1024 bytes");
    }
    memory.forget();
    let query = query.trim();
    let needle = query.to_lowercase();
    let mut candidates: Vec<(bool, SymbolNode)> = api::declared(elf, memory)
        .into_iter()
        .map(|(_, node)| (false, node))
        .collect();
    candidates.extend(
        tree::roots(elf)
            .into_iter()
            .filter(|root| {
                root.node.readable
                    && !matches!(
                        root.section.as_str(),
                        ".defmt" | ".rtt" | ".vector_table" | ".embedded_test"
                    )
                    && (!root.node.path.ends_with("::{vtable}") || needle.contains("vtable"))
            })
            .map(|root| {
                // Application-owned StaticCells are useful roots despite their runtime wrapper.
                let internal = root.internal && !root.node.type_name.starts_with("StaticCell<");
                (internal, root.node)
            }),
    );
    candidates.extend(
        tasks::tasks(elf)
            .into_iter()
            .map(|task| (false, task.root.node)),
    );
    candidates.retain(|(_, node)| {
        node.path.len() <= 1024
            && (node.path.to_lowercase().contains(&needle)
                || node.type_name.to_lowercase().contains(&needle))
    });
    if let Some((parent, fragment)) = split_query(query) {
        let resolved = path::locate(Some(elf), memory, parent);
        if let Ok(path::Location::Node(parent)) = resolved {
            let active = if live && parent.kind == NodeKind::TaggedEnum {
                tree::preview(elf, &parent.node, &mut |address, len| {
                    memory.read(address, len).map_err(|e| format!("{e:#}"))
                })
                .ok()
                .flatten()
                .and_then(|preview| preview.active_variant)
            } else {
                None
            };
            let offset = fragment.trim_end_matches(']').parse::<u64>().unwrap_or(0);
            let page = tree::children_page(
                elf,
                &parent.node,
                offset,
                128,
                &mut |address, len| memory.word(address, len).map_err(|e| format!("{e:#}")),
                live,
            );
            if let Ok(page) = page {
                for child in page.nodes {
                    if matches!(child.node.steps.last(), Some(Step::Discriminant)) {
                        continue;
                    }
                    if let Some(Step::Variant(name)) = child.node.steps.last()
                        && live
                        && active.as_ref() != Some(name)
                    {
                        continue;
                    }
                    if child.path.len() <= 1024
                        && child
                            .label
                            .to_lowercase()
                            .contains(&fragment.to_lowercase())
                    {
                        candidates.push((false, child));
                    }
                }
            }
        }
    }
    candidates.sort_by(|(a_internal, a), (b_internal, b)| {
        a_internal
            .cmp(b_internal)
            .then_with(|| {
                (!a.label.to_lowercase().starts_with(&needle))
                    .cmp(&!b.label.to_lowercase().starts_with(&needle))
            })
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut seen = std::collections::HashSet::new();
    let items = candidates
        .into_iter()
        .filter(|(_, node)| seen.insert(node.path.clone()))
        .take(LIMIT)
        .map(|(_, node)| Item {
            type_name: wrappers::type_name(&node),
            path: node.path,
            label: node.label,
        })
        .collect();
    serde_json::to_string(&Completions { version: 1, items }).map_err(|error| anyhow!(error))
}

/// Last field/index separator outside Rust generic arguments and completed indices.
fn split_query(query: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    let mut bracket = 0;
    let mut last = None;
    for (i, c) in query.char_indices() {
        match c {
            '<' | '(' | '{' => depth += 1,
            '>' | ')' | '}' => depth -= 1,
            '[' if depth == 0 => {
                if bracket == 0 {
                    last = Some(i);
                }
                bracket += 1;
            }
            ']' if depth == 0 => bracket -= 1,
            '.' | '#' if depth == 0 && bracket == 0 => last = Some(i),
            _ => {}
        }
    }
    last.map(|i| (&query[..i], &query[i + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Fake;
    use studio_dwarf::ElfParser;

    const ELF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wrappers.elf");

    #[test]
    fn completes_roots_fields_and_arrays_without_reading_values() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let result = |memory: &mut Fake, query| -> serde_json::Value {
            serde_json::from_str(&complete(&elf, memory, query).unwrap()).unwrap()
        };
        let roots = result(&mut memory, "ato");
        assert!(
            roots["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["path"] == "wrapper_fixture::ATOMIC")
        );
        let fields = result(&mut memory, "wrapper_fixture::DATA.g");
        assert_eq!(fields["items"][0]["path"], "wrapper_fixture::DATA.gain");
        assert_eq!(fields["items"][0]["type"], "f32");
        assert!(!fields.to_string().contains("display"));
        let indices = result(&mut memory, "wrapper_fixture::HEAP[199");
        assert!(
            indices["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["path"] == "wrapper_fixture::HEAP[199]")
        );

        let offline: serde_json::Value =
            serde_json::from_str(&complete_metadata(&elf, "wrapper_fixture::DATA.g").unwrap())
                .unwrap();
        assert_eq!(offline["items"][0]["path"], "wrapper_fixture::DATA.gain");
        assert!(
            result(&mut memory, "not_a_symbol")["items"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(complete(&elf, &mut memory, &"x".repeat(1025)).is_err());
    }

    #[test]
    fn separators_ignore_names_inside_generic_arguments_and_task_indices() {
        assert_eq!(
            split_query("<a::B<[u8; 3]> as a::C>::VALUE.field"),
            Some(("<a::B<[u8; 3]> as a::C>::VALUE", "field"))
        );
        assert_eq!(
            split_query("task::POOL[task 0].future#3.tick"),
            Some(("task::POOL[task 0].future#3", "tick"))
        );
        assert_eq!(split_query("BUFFER[99"), Some(("BUFFER", "99")));
    }
}
