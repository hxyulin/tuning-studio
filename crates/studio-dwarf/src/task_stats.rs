//! Per-task CPU time counted by the firmware. rm-embedded-rs' `rm-task-stats`
//! implements embassy's trace hooks and keeps, for each spawned task, its poll
//! count, the cycles spent polling it and its longest poll, in one static
//! (`RM_TASK_STATS`). Its fields are found here by name from DWARF; the whole
//! static is read in one go, and a slot is matched to a task by the address of
//! its `TaskHeader`, which is the task slot's address.

use serde::Serialize;

use crate::elf::ElfInfo;
use crate::tree::{self, NodeKind, NodeRef, Step, TreeError};

/// `TaskStats::magic`: `RMTS` in little-endian byte order.
pub const STATS_MAGIC: u32 = u32::from_le_bytes(*b"RMTS");
pub const STATS_VERSION: u32 = 1;
/// More slots than this is not a table this tool wrote
const MAX_SLOTS: u64 = 1024;

/// Where `RM_TASK_STATS` and its fields are.
#[derive(Debug, Clone)]
pub struct StatsLayout {
    pub symbol: String,
    pub address: u64,
    size: usize,
    little_endian: bool,
    magic: u64,
    version: u64,
    clock_hz: u64,
    untracked: u64,
    slots: u64,
    stride: u64,
    count: u64,
    slot: SlotLayout,
}

/// Field offsets within one slot
#[derive(Debug, Clone, Copy)]
struct SlotLayout {
    task: u64,
    polls: u64,
    cycles: u64,
    max_cycles: u64,
    peak: u64,
    last_peak: u64,
}

/// One read of `RM_TASK_STATS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsReading {
    /// Cycle counter frequency; 0 until the firmware calls `init`
    pub clock_hz: u32,
    /// Tasks spawned while every slot was taken
    pub untracked: u32,
    /// Slots in use
    pub slots: Vec<TaskCounters>,
}

/// One task's counters. Cycle and poll counts wrap at 32 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCounters {
    /// The task's `TaskHeader` address, as 32 bits
    pub task: u32,
    pub polls: u32,
    pub cycles: u32,
    /// Longest poll since the task was spawned
    pub max_cycles: u32,
    /// Longest poll in the last one to two seconds
    pub recent_max_cycles: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error("{0}")]
    Tree(#[from] TreeError),
    #[error("RM_TASK_STATS: {0}")]
    Layout(String),
}

impl StatsLayout {
    /// The firmware's task counters, or `None` when it does not link `rm-task-stats`.
    pub fn find(elf: &ElfInfo) -> Result<Option<Self>, StatsError> {
        let Some(sym) = elf.variables().into_iter().find(|v| {
            v.demangled_name == "RM_TASK_STATS" || v.demangled_name.ends_with("::RM_TASK_STATS")
        }) else {
            return Ok(None);
        };
        let root = NodeRef::root(&sym.demangled_name);
        let base = sym.address;
        let field = |parent: &NodeRef, name: &str, at: u64| -> Result<u64, StatsError> {
            let node = tree::node(elf, &parent.child(Step::Member(name.into())))?;
            if node.size != Some(4) {
                return Err(StatsError::Layout(format!(
                    "`{name}` is {:?} bytes, expected 4",
                    node.size
                )));
            }
            Ok(node.address - at)
        };

        let slots_ref = root.child(Step::Member("slots".into()));
        let slots = tree::node(elf, &slots_ref)?;
        let count = match (slots.kind, slots.child_count) {
            (NodeKind::Array, Some(n)) if (1..=MAX_SLOTS).contains(&n) => n,
            _ => {
                return Err(StatsError::Layout(format!(
                    "`slots` is not an array of at most {MAX_SLOTS}"
                )));
            }
        };
        let stride = slots.size.unwrap_or(0) / count;
        let first = slots_ref.child(Step::Index(0));
        let slot_at = slots.address;
        let slot = SlotLayout {
            task: field(&first, "task", slot_at)?,
            polls: field(&first, "polls", slot_at)?,
            cycles: field(&first, "cycles", slot_at)?,
            max_cycles: field(&first, "max_cycles", slot_at)?,
            peak: field(&first, "peak", slot_at)?,
            last_peak: field(&first, "last_peak", slot_at)?,
        };
        let size = sym.size.max(slots.address - base + stride * count);
        Ok(Some(Self {
            symbol: sym.demangled_name.clone(),
            address: base,
            size: usize::try_from(size).map_err(|_| StatsError::Layout("too large".into()))?,
            little_endian: elf.is_little_endian,
            magic: field(&root, "magic", base)?,
            version: field(&root, "version", base)?,
            clock_hz: field(&root, "clock_hz", base)?,
            untracked: field(&root, "untracked", base)?,
            slots: slots.address - base,
            stride,
            count,
            slot,
        }))
    }

    /// The memory to read: the whole static
    pub fn region(&self) -> (u64, usize) {
        (self.address, self.size)
    }

    /// Decode what [`Self::region`] read.
    pub fn decode(&self, bytes: &[u8]) -> Result<StatsReading, String> {
        let word = |at: u64| -> Result<u32, String> {
            let at = usize::try_from(at).map_err(|e| e.to_string())?;
            let b: [u8; 4] = bytes
                .get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .ok_or("short read")?;
            Ok(if self.little_endian {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            })
        };
        let magic = word(self.magic)?;
        if magic != STATS_MAGIC {
            return Err(if magic == 0 {
                "not started yet".into()
            } else {
                format!("magic is {magic:#010x}; the firmware has not reset it")
            });
        }
        let version = word(self.version)?;
        if version != STATS_VERSION {
            return Err(format!(
                "format version {version}, this tool reads {STATS_VERSION}"
            ));
        }
        let mut slots = Vec::new();
        for i in 0..self.count {
            let at = self.slots + i * self.stride;
            let task = word(at + self.slot.task)?;
            if task == 0 {
                continue;
            }
            slots.push(TaskCounters {
                task,
                polls: word(at + self.slot.polls)?,
                cycles: word(at + self.slot.cycles)?,
                max_cycles: word(at + self.slot.max_cycles)?,
                recent_max_cycles: word(at + self.slot.peak)?.max(word(at + self.slot.last_peak)?),
            });
        }
        Ok(StatsReading {
            clock_hz: word(self.clock_hz)?,
            untracked: word(self.untracked)?,
            slots,
        })
    }
}

impl StatsReading {
    /// The counters of the task whose slot starts at `address`
    pub fn task(&self, address: u64) -> Option<&TaskCounters> {
        self.slots
            .iter()
            .find(|s| u64::from(s.task) == address & 0xffff_ffff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::ElfParser;

    const EMBASSY_ELF: &[u8] = include_bytes!("../tests/fixtures/embassy_tasks.elf");

    fn layout() -> StatsLayout {
        let elf = ElfParser::parse_bytes(EMBASSY_ELF, "embassy_tasks.elf").unwrap();
        StatsLayout::find(&elf)
            .unwrap()
            .expect("fixture links rm-task-stats")
    }

    fn put(bytes: &mut [u8], at: u64, value: u32) {
        let at = at as usize;
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn finds_the_table_by_field_name() {
        let layout = layout();
        assert!(layout.symbol.ends_with("rm_task_stats::RM_TASK_STATS"));
        assert_eq!(layout.count, 32);
        assert_eq!(layout.stride, 28);
        assert_eq!(layout.region().1, 24 + 32 * 28);
    }

    #[test]
    fn decodes_slots_in_use() {
        let layout = layout();
        let mut bytes = vec![0u8; layout.region().1];
        assert_eq!(layout.decode(&bytes).unwrap_err(), "not started yet");

        put(&mut bytes, layout.magic, STATS_MAGIC);
        put(&mut bytes, layout.version, STATS_VERSION);
        put(&mut bytes, layout.clock_hz, 520_000_000);
        let second = layout.slots + layout.stride;
        put(&mut bytes, second + layout.slot.task, 0x2000_0100);
        put(&mut bytes, second + layout.slot.polls, 7);
        put(&mut bytes, second + layout.slot.cycles, 7000);
        put(&mut bytes, second + layout.slot.max_cycles, 4000);
        put(&mut bytes, second + layout.slot.peak, 300);
        put(&mut bytes, second + layout.slot.last_peak, 900);

        let reading = layout.decode(&bytes).unwrap();
        assert_eq!(reading.clock_hz, 520_000_000);
        assert_eq!(
            reading.slots,
            [TaskCounters {
                task: 0x2000_0100,
                polls: 7,
                cycles: 7000,
                max_cycles: 4000,
                recent_max_cycles: 900,
            }]
        );
        assert!(reading.task(0x2000_0100).is_some());
        assert!(reading.task(0x2000_0200).is_none());

        put(&mut bytes, layout.version, 2);
        assert!(layout.decode(&bytes).unwrap_err().contains("version 2"));
        assert_eq!(layout.decode(&bytes[..4]).unwrap_err(), "short read");
    }
}
