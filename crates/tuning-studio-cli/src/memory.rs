use anyhow::{Result, bail};
use std::ops::Range;

/// Ranges closer than this are read in one transfer. With a CMSIS-DAP probe at
/// 10 MHz SWD, a transfer cost about 1.07 ms plus 3.6 µs per byte, so a gap
/// costs as much as a second transfer at about 300 bytes.
pub const MERGE_GAP: u64 = 128;

/// Target memory. Cortex-M targets are little-endian.
pub trait Memory {
    /// Read `len` bytes at `address` from the target now.
    fn transfer(&mut self, address: u64, len: usize) -> Result<Vec<u8>>;
    /// Write `data` at `address`, and drop every snapshot.
    fn write(&mut self, address: u64, data: &[u8]) -> Result<()>;
    fn snapshots(&mut self) -> &mut Snapshots;
    /// RAM, where reading the bytes between two values has no side effects
    fn ram(&self) -> &[Range<u64>];

    /// `len` bytes at `address`, from a snapshot that holds them or else from the
    /// target.
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
        if let Some(bytes) = self.snapshots().get(address, len) {
            return Ok(bytes.to_vec());
        }
        self.transfer(address, len)
    }

    /// Read `len` bytes at `address` in one transfer, and serve later reads inside
    /// that range from them, so a struct's fields come from the same moment. During
    /// a [`prefetch`] that already holds them, it reads nothing.
    fn snapshot(&mut self, address: u64, len: usize) -> Result<()> {
        if self.snapshots().batch && self.snapshots().get(address, len).is_some() {
            return Ok(());
        }
        self.reread(address, len)
    }

    /// As [`snapshot`](Memory::snapshot), but always from the target, for a retry.
    fn reread(&mut self, address: u64, len: usize) -> Result<()> {
        let bytes = self.transfer(address, len)?;
        self.snapshots().insert(address, bytes);
        Ok(())
    }

    /// Drop every snapshot, so later reads go to the target.
    fn forget(&mut self) {
        *self.snapshots() = Snapshots::default();
    }

    /// Unsigned value of 1 to 8 bytes.
    fn word(&mut self, address: u64, len: usize) -> Result<u64> {
        if !(1..=8).contains(&len) {
            bail!("unsupported word size {len}");
        }
        let mut raw = [0; 8];
        raw[..len].copy_from_slice(&self.read(address, len)?);
        Ok(u64::from_le_bytes(raw))
    }
}

/// Read every range now, nearby ones in one transfer, so the values they hold
/// cost one round trip together. Snapshots inside them use these bytes until
/// [`Memory::forget`].
pub fn prefetch(
    memory: &mut dyn Memory,
    ranges: impl IntoIterator<Item = (u64, usize)>,
) -> Result<()> {
    memory.forget();
    memory.snapshots().batch = true;
    for (address, len) in merge(ranges, memory.ram()) {
        memory.reread(address, len)?;
    }
    Ok(())
}

/// The ranges as reads. Ranges less than [`MERGE_GAP`] apart in the same RAM
/// region are joined; the rest, such as peripheral registers, are read alone.
pub fn merge(
    ranges: impl IntoIterator<Item = (u64, usize)>,
    ram: &[Range<u64>],
) -> Vec<(u64, usize)> {
    let mut ranges: Vec<(u64, u64)> = ranges
        .into_iter()
        .filter(|&(_, len)| len > 0)
        .map(|(address, len)| (address, address + len as u64))
        .collect();
    ranges.sort();
    let mut reads: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        match reads.last_mut() {
            Some(last)
                if start <= last.1 + MERGE_GAP
                    && ram
                        .iter()
                        .any(|r| r.start <= last.0 && end.max(last.1) <= r.end) =>
            {
                last.1 = last.1.max(end)
            }
            _ => reads.push((start, end)),
        }
    }
    reads
        .into_iter()
        .map(|(start, end)| (start, (end - start) as usize))
        .collect()
}

/// Bytes read in one transfer each
#[derive(Default)]
pub struct Snapshots {
    regions: Vec<(u64, Vec<u8>)>,
    /// Set by a prefetch: snapshots inside the regions use them, and a new one
    /// joins them rather than replacing them
    batch: bool,
}

impl Snapshots {
    fn get(&self, address: u64, len: usize) -> Option<&[u8]> {
        self.regions.iter().find_map(|(start, bytes)| {
            let offset = usize::try_from(address.checked_sub(*start)?).ok()?;
            bytes.get(offset..offset.checked_add(len)?)
        })
    }

    /// Newer bytes come first, so a retry's read wins over the batch it repeats,
    /// while other values keep the batch's bytes.
    fn insert(&mut self, address: u64, bytes: Vec<u8>) {
        if !self.batch {
            self.regions.clear();
        }
        self.regions.insert(0, (address, bytes));
    }
}

/// Memory backed by buffers, for tests. It counts its transfers.
#[cfg(test)]
pub struct Fake {
    regions: Vec<(u64, Vec<u8>)>,
    ram: Vec<Range<u64>>,
    snapshots: Snapshots,
    pub transfers: usize,
}

#[cfg(test)]
impl Fake {
    pub fn new(base: u64, bytes: Vec<u8>) -> Self {
        Self {
            ram: std::iter::once(base..base + bytes.len() as u64).collect(),
            regions: vec![(base, bytes)],
            snapshots: Snapshots::default(),
            transfers: 0,
        }
    }

    /// Every section the ELF loads, as the target holds it after startup: initialized
    /// data, zeroed `.bss`, and flash.
    pub fn from_elf(path: &str) -> Self {
        use object::{Object, ObjectSection, SectionKind};
        let data = std::fs::read(path).unwrap();
        let file = object::File::parse(&*data).unwrap();
        let regions = file
            .sections()
            // Debug sections are not loaded, and have address 0.
            .filter(|s| s.address() != 0)
            .map(|s| {
                let bytes = match s.kind() {
                    SectionKind::UninitializedData => vec![0; s.size() as usize],
                    _ => s.data().unwrap().to_vec(),
                };
                (s.address(), bytes)
            })
            .collect::<Vec<_>>();
        Self {
            ram: regions
                .iter()
                .map(|(base, bytes)| *base..*base + bytes.len() as u64)
                .collect(),
            regions,
            snapshots: Snapshots::default(),
            transfers: 0,
        }
    }

    fn region(&mut self, address: u64, len: usize) -> Result<&mut [u8]> {
        self.regions
            .iter_mut()
            .find_map(|(base, bytes)| {
                let start = address.checked_sub(*base)? as usize;
                bytes.get_mut(start..start.checked_add(len)?)
            })
            .ok_or_else(|| anyhow::anyhow!("{address:#x} is outside fake memory"))
    }
}

#[cfg(test)]
impl Memory for Fake {
    fn transfer(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
        self.transfers += 1;
        Ok(self.region(address, len)?.to_vec())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        self.forget();
        self.region(address, data.len())?.copy_from_slice(data);
        Ok(())
    }

    fn snapshots(&mut self) -> &mut Snapshots {
        &mut self.snapshots
    }

    fn ram(&self) -> &[Range<u64>] {
        &self.ram
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM: &[Range<u64>] = &[0..0x1000, 0x2000_0000..0x2002_0000];

    #[test]
    fn merges_nearby_ranges() {
        assert_eq!(
            merge(
                [
                    (0x500, 4),
                    (0x100, 16),
                    (0x108, 4),
                    (0x110 + MERGE_GAP, 8),
                    (0x900, 0)
                ],
                RAM
            ),
            [(0x100, 0x18 + MERGE_GAP as usize), (0x500, 4)]
        );
        // Not across the end of RAM, nor outside it
        assert_eq!(
            merge([(0xff0, 8), (0x1000, 8)], RAM),
            [(0xff0, 8), (0x1000, 8)]
        );
        assert_eq!(merge([(0x4000_0000, 4), (0x4000_0004, 4)], RAM).len(), 2);
    }

    #[test]
    fn prefetched_snapshots_read_nothing() {
        let mut memory = Fake::new(0x1000, (0..64).collect());
        prefetch(&mut memory, [(0x1000, 8), (0x1010, 8)]).unwrap();
        assert_eq!(memory.transfers, 1);
        memory.snapshot(0x1010, 8).unwrap();
        assert_eq!(memory.read(0x1012, 2).unwrap(), [0x12, 0x13]);
        assert_eq!(memory.transfers, 1);
        // A retry reads again: its bytes win, and other values keep the batch's.
        memory.region(0x1000, 1).unwrap()[0] = 0xaa;
        memory.region(0x1010, 1).unwrap()[0] = 0xbb;
        memory.reread(0x1000, 4).unwrap();
        memory.snapshot(0x1010, 8).unwrap();
        assert_eq!(memory.transfers, 2);
        assert_eq!(memory.read(0x1000, 1).unwrap(), [0xaa]);
        assert_eq!(memory.read(0x1010, 1).unwrap(), [0x10]);
        // Outside a batch, each snapshot reads.
        memory.forget();
        memory.snapshot(0x1010, 8).unwrap();
        memory.snapshot(0x1010, 8).unwrap();
        assert_eq!(memory.transfers, 4);
    }
}
