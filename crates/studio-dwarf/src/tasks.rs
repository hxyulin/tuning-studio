//! embassy tasks. `#[embassy_executor::task]` keeps each task's futures in a
//! `POOL` static of untyped bytes (`TaskPoolHolder<SIZE, ALIGN>`), but the DWARF
//! still describes the `TaskStorage<F>` for the task's future type `F`. A pool
//! slot can therefore be browsed as a `TaskStorage` (see [`Step::Task`]), and
//! a few bytes of it tell whether the task is spawned, whether it is queued to
//! run, and which `.await` it is parked on.
//!
//! The future is the compiler's state machine for the `async fn`: a tagged enum
//! with `Unresumed` (holding the arguments), `Returned`, `Panicked`, and one
//! `SuspendN` per `.await`, holding the locals that live across it. The DWARF
//! records the source line of each `.await`.

use serde::Serialize;

use crate::elf::ElfInfo;
use crate::tree::{self, NodeKind, NodeRef, RootNode, Step, SymbolNode, TreeError, split_path};
use crate::type_table::{SourceLocation, TypeDef, TypeId, TypeTable};

/// One slot of a task pool: a task that can run once at a time.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// The slot as a tree root; its label is the task name, its segments the
    /// module path, and its children the `TaskStorage` members
    pub root: RootNode,
    /// The task function's name; `main` for `#[embassy_executor::main]`
    pub name: String,
    pub slot: u64,
    /// Slots in the pool: `pool_size` in `#[task(pool_size = N)]`
    pub slots: u64,
    /// Bytes of the `async fn`'s future: its arguments and the locals it keeps
    /// across an `.await`. embassy tasks share one stack, so this is a task's RAM.
    pub future_size: Option<u64>,
    /// The states the future can be in, in declaration order
    pub states: Vec<TaskPoint>,
}

/// The storage type behind a task pool.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Storage {
    pub type_id: TypeId,
    /// One slot's size; the pool is `slots` of them back to back
    pub size: u64,
    pub slots: u64,
}

/// Every embassy task slot in the ELF, sorted by path then slot.
pub fn tasks(elf: &ElfInfo) -> Vec<Task> {
    let table = elf.type_table();
    let mut out = Vec::new();
    for sym in elf.get_variables() {
        let Some(storage) = storage_type(table, &sym.demangled_name, sym.size) else {
            continue;
        };
        let Some((module, name)) = task_name(&sym.demangled_name) else {
            continue;
        };
        let name = if name == "__embassy_main" {
            "main".to_string()
        } else {
            name
        };
        for slot in 0..storage.slots {
            let label = if storage.slots > 1 {
                format!("{name} [{slot}]")
            } else {
                name.clone()
            };
            let mut node = tree::describe(
                table,
                NodeRef::root(&sym.demangled_name).child(Step::Task(slot)),
                label.clone(),
                sym.address + slot * storage.size,
                storage.type_id,
            );
            node.readable = sym.is_readable();
            node.status = sym.unreadable_reason().map(str::to_string);
            let mut segments = module.clone();
            segments.push(label);
            let future = tree::node(elf, &node.node.child(Step::Member("future".into())));
            let states = states(elf, &node.node.child(Step::Member("future".into())))
                .map(|(_, points)| points.into_iter().map(|(_, p)| p).collect())
                .unwrap_or_default();
            out.push(Task {
                root: RootNode {
                    node,
                    segments,
                    section: sym.section.clone(),
                    read_only: !sym.writable,
                    internal: false,
                },
                name: name.clone(),
                slot,
                slots: storage.slots,
                future_size: future.ok().and_then(|f| f.size),
                states,
            });
        }
    }
    out.sort_by(|a, b| {
        a.root
            .node
            .path
            .cmp(&b.root.node.path)
            .then(a.slot.cmp(&b.slot))
    });
    out.dedup_by(|a, b| a.root.node.path == b.root.node.path);
    out
}

/// `app::led_task::POOL` is task `led_task` in module `app`.
fn task_name(pool: &str) -> Option<(Vec<String>, String)> {
    let mut segments = split_path(pool);
    if segments.len() < 2 || segments.pop()? != "POOL" {
        return None;
    }
    let name = segments.pop()?;
    Some((segments, name))
}

/// The `TaskStorage` a pool of `pool_size` bytes at `pool` holds.
///
/// The task macro names the future after the task: `app::led_task::POOL` holds
/// `TaskStorage<app::__led_task_task::__led_task_task_inner_function::{async_fn_env#0}>`.
/// Types of that name are defined once per compile unit using them; one whose
/// size divides the pool is the one.
pub(crate) fn storage_type(table: &TypeTable, pool: &str, pool_size: u64) -> Option<Storage> {
    let (module, name) = task_name(pool)?;
    let prefix: String = module.iter().map(|m| format!("{m}::")).collect();
    let inner = format!("{prefix}__{name}_task::__{name}_task_inner_function");
    ["{async_fn_env#0}", "{closure#0}"]
        .iter()
        .flat_map(|env| {
            let future = format!("{inner}::{env}");
            [
                format!("TaskStorage<{future}>"),
                format!("embassy_executor::raw::TaskStorage<{future}>"),
            ]
        })
        .flat_map(|candidate| table.find_by_name(&candidate))
        .find_map(|id| match table.get(id) {
            Some(TypeDef::Struct(s)) if s.size > 0 && pool_size.is_multiple_of(s.size) => {
                Some(Storage {
                    type_id: id,
                    size: s.size,
                    slots: pool_size / s.size,
                })
            }
            _ => None,
        })
}

/// Where to read a task's state, and how to decode it.
#[derive(Debug, Clone)]
pub struct TaskProbe {
    spawned: Flag,
    queued: Flag,
    tag: Option<Tag>,
}

/// One bit of the task header's state
#[derive(Debug, Clone, Copy)]
struct Flag {
    address: u64,
    mask: u8,
}

/// The future's discriminant and the variants it selects
#[derive(Debug, Clone)]
struct Tag {
    address: u64,
    size: usize,
    little_endian: bool,
    points: Vec<VariantPoint>,
}

/// A live task's state, decoded from [`TaskProbe::regions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    /// Spawned and not finished: the pool slot holds a future
    pub spawned: bool,
    /// In the executor's run queue: woken, waiting for its turn to be polled
    pub queued: bool,
    /// Which state the future is in; `None` when not spawned
    pub at: Option<TaskPoint>,
}

/// One state of an `async fn`'s future.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPoint {
    /// `Unresumed`, `Returned`, `Panicked`, or `Suspend0`, `Suspend1`, …
    pub label: String,
    /// The variant's node path, to find it in the tree
    pub path: String,
    /// The variant node; its children are the state's locals
    #[serde(rename = "ref")]
    pub node: NodeRef,
    /// The `.await` a suspended task is parked on
    pub location: Option<SourceLocation>,
}

/// How to read the state of the task slot `task` (a [`Step::Task`] reference).
pub fn probe(elf: &ElfInfo, task: &NodeRef) -> Result<TaskProbe, TreeError> {
    let state = tree::node(
        elf,
        &task
            .child(Step::Member("raw".into()))
            .child(Step::Member("state".into())),
    )?;
    let (spawned, queued) = if state.kind == NodeKind::Scalar {
        // `AtomicU32` / `AtomicU8` holding bit flags; bits 0 and 1 are in the
        // lowest byte on a little-endian target
        (
            Flag {
                address: state.address,
                mask: 1,
            },
            Flag {
                address: state.address,
                mask: 2,
            },
        )
    } else {
        // Cortex-M: one `AtomicBool` per flag
        let flag = |member: &str| {
            tree::node(elf, &state.node.child(Step::Member(member.into()))).map(|n| Flag {
                address: n.address,
                mask: 0xff,
            })
        };
        (flag("spawned")?, flag("run_queued")?)
    };

    let future = task.child(Step::Member("future".into()));
    let tag = states(elf, &future).and_then(|(discriminant, points)| {
        Some(Tag {
            address: discriminant.address,
            size: usize::try_from(discriminant.size?)
                .ok()
                .filter(|s| (1..=8).contains(s))?,
            little_endian: elf.is_little_endian,
            points,
        })
    });
    Ok(TaskProbe {
        spawned,
        queued,
        tag,
    })
}

/// A future state and the tag value that selects it; `None` for the default variant
type VariantPoint = (Option<u64>, TaskPoint);

/// The future's discriminant and its variants, keyed by the tag value that
/// selects each (`None` for the default variant). `None` when the future is
/// not a tagged enum.
fn states(elf: &ElfInfo, future: &NodeRef) -> Option<(SymbolNode, Vec<VariantPoint>)> {
    let node = tree::node(elf, future).ok()?;
    if node.kind != NodeKind::TaggedEnum {
        return None;
    }
    let children = tree::children(elf, future, None).ok()?;
    let mut discriminant = None;
    let mut points = Vec::new();
    for c in children.nodes {
        match c.node.steps.last() {
            Some(Step::Discriminant) => discriminant = Some(c),
            Some(Step::Variant(_)) => points.push((
                c.discr_value,
                TaskPoint {
                    label: c.label,
                    path: c.path,
                    node: c.node,
                    location: c.location,
                },
            )),
            _ => {}
        }
    }
    Some((discriminant?, points))
}

impl TaskProbe {
    /// Memory to read, as `(address, length)`; pass the bytes to [`Self::decode`]
    /// in the same order.
    pub fn regions(&self) -> Vec<(u64, usize)> {
        let start = self.spawned.address.min(self.queued.address);
        let end = self.spawned.address.max(self.queued.address) + 1;
        let mut regions = vec![(start, (end - start) as usize)];
        if let Some(tag) = &self.tag {
            regions.push((tag.address, tag.size));
        }
        regions
    }

    /// Decode what [`Self::regions`] read. `None` when a region is missing or short.
    pub fn decode(&self, bytes: &[Vec<u8>]) -> Option<TaskState> {
        let state = bytes.first()?;
        let start = self.spawned.address.min(self.queued.address);
        let flag = |f: Flag| -> Option<bool> {
            let byte = state.get(usize::try_from(f.address - start).ok()?)?;
            Some(byte & f.mask != 0)
        };
        let spawned = flag(self.spawned)?;
        let queued = flag(self.queued)?;
        // The future is uninitialized memory until the task is spawned
        let at = match (&self.tag, spawned) {
            (Some(tag), true) => {
                let raw = bytes.get(1)?.get(..tag.size)?;
                let mut word = [0u8; 8];
                if tag.little_endian {
                    word[..tag.size].copy_from_slice(raw);
                } else {
                    word[8 - tag.size..].copy_from_slice(raw);
                }
                let value = if tag.little_endian {
                    u64::from_le_bytes(word)
                } else {
                    u64::from_be_bytes(word)
                };
                tag.points
                    .iter()
                    .find(|(d, _)| *d == Some(value))
                    .or_else(|| tag.points.iter().find(|(d, _)| d.is_none()))
                    .map(|(_, point)| point.clone())
            }
            _ => None,
        };
        Some(TaskState {
            spawned,
            queued: spawned && queued,
            at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::ElfParser;
    use crate::tree::children;

    const EMBASSY_ELF: &[u8] = include_bytes!("../tests/fixtures/embassy_tasks.elf");

    fn elf() -> ElfInfo {
        ElfParser::parse_bytes(EMBASSY_ELF, "embassy_tasks.elf").unwrap()
    }

    fn find<'t>(tasks: &'t [Task], label: &str) -> &'t Task {
        tasks
            .iter()
            .find(|t| t.root.node.label == label)
            .unwrap_or_else(|| panic!("no task {label}"))
    }

    #[test]
    fn pools_become_task_slots() {
        let elf = elf();
        let tasks = tasks(&elf);
        let labels: Vec<&str> = tasks.iter().map(|t| t.root.node.label.as_str()).collect();
        assert_eq!(labels, ["main", "blink_task", "worker [0]", "worker [1]"]);

        let blink = find(&tasks, "blink_task");
        assert_eq!(
            blink.root.segments,
            ["embassy_fixture", "blink", "blink_task"]
        );
        assert_eq!(
            blink.root.node.path,
            "embassy_fixture::blink::blink_task::POOL[task 0]"
        );
        assert!(blink.root.node.type_name.starts_with("TaskStorage<"));
        assert!(!blink.root.internal);

        let pool = elf.find_symbol("embassy_fixture::worker::POOL").unwrap();
        let second = find(&tasks, "worker [1]");
        assert_eq!((second.slot, second.slots), (1, 2));
        assert_eq!(second.root.node.address, pool.address + pool.size / 2);

        // The future's size and states come without a probe
        assert!(blink.future_size.is_some_and(|s| s > 0));
        let states: Vec<&str> = blink.states.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            states,
            ["Unresumed", "Returned", "Panicked", "Suspend0", "Suspend1"]
        );
        assert_eq!(blink.states[3].location.as_ref().unwrap().line, 47);
    }

    #[test]
    fn a_task_browses_as_its_storage() {
        let elf = elf();
        let blink = find(&tasks(&elf), "blink_task").root.node.node.clone();
        let storage = children(&elf, &blink, None).unwrap();
        let labels: Vec<&str> = storage.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, ["raw", "future"]);

        // The future is the `async fn`'s state machine, shown through `UninitCell`
        let future = &storage.nodes[1];
        assert_eq!(future.kind, NodeKind::TaggedEnum);
        let states = children(&elf, &future.node, None).unwrap();
        let labels: Vec<&str> = states.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "<discriminant>",
                "Unresumed",
                "Returned",
                "Panicked",
                "Suspend0",
                "Suspend1"
            ]
        );
        let suspend0 = &states.nodes[4];
        let at = suspend0.location.as_ref().unwrap();
        assert!(at.file.ends_with("src/main.rs"), "{}", at.file);
        assert_eq!(at.line, 47);

        // The arguments live in `Unresumed`, locals across an await in `SuspendN`
        let args = children(&elf, &states.nodes[1].node, None).unwrap();
        assert!(
            args.nodes.iter().any(|n| n.label == "period"),
            "{:?}",
            args.nodes
        );
        let locals = children(&elf, &suspend0.node, None).unwrap();
        assert!(
            locals.nodes.iter().any(|n| n.label == "count"),
            "{:?}",
            locals.nodes
        );
    }

    #[test]
    fn bad_task_slots_are_errors() {
        let elf = elf();
        let pool = NodeRef::root("embassy_fixture::worker::POOL");
        assert!(tree::node(&elf, &pool.child(Step::Task(1))).is_ok());
        let err = tree::node(&elf, &pool.child(Step::Task(2))).unwrap_err();
        assert!(err.to_string().contains("out of bounds"), "{err}");
        let err = tree::node(
            &elf,
            &NodeRef::root("embassy_fixture::TICKS").child(Step::Task(0)),
        )
        .unwrap_err();
        assert!(err.to_string().contains("task pool"), "{err}");
    }

    #[test]
    fn state_decodes_from_header_and_tag() {
        let elf = elf();
        let blink = find(&tasks(&elf), "blink_task").root.node.node.clone();
        let probe = probe(&elf, &blink).unwrap();
        let regions = probe.regions();
        assert_eq!(regions.len(), 2);
        let (state_at, state_len) = regions[0];
        let (_, tag_len) = regions[1];
        let state = |spawned: bool, queued: bool| {
            let mut b = vec![0u8; state_len];
            let flags = [(probe.spawned, spawned), (probe.queued, queued)];
            for (f, on) in flags {
                if on {
                    b[(f.address - state_at) as usize] |= f.mask;
                }
            }
            b
        };
        let tag = |v: u8| {
            let mut b = vec![0u8; tag_len];
            b[0] = v;
            b
        };

        let idle = probe.decode(&[state(false, false), tag(4)]).unwrap();
        assert_eq!(
            idle,
            TaskState {
                spawned: false,
                queued: false,
                at: None
            }
        );

        let parked = probe.decode(&[state(true, false), tag(4)]).unwrap();
        let at = parked.at.unwrap();
        assert_eq!(at.label, "Suspend1");
        assert_eq!(at.location.unwrap().line, 49);
        assert!(at.path.ends_with("POOL[task 0].future#4"), "{}", at.path);
        assert!(tree::node(&elf, &blink).is_ok());

        let ready = probe.decode(&[state(true, true), tag(0)]).unwrap();
        assert!(ready.queued);
        assert_eq!(ready.at.unwrap().label, "Unresumed");

        assert_eq!(probe.decode(&[state(true, false)]), None);
    }
}
