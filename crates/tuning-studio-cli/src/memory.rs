use anyhow::{Result, bail};

/// Target memory. Cortex-M targets are little-endian.
pub trait Memory {
    /// Read `len` bytes at `address` now, and serve later reads inside that range
    /// from them, so a struct's fields come from the same moment.
    fn snapshot(&mut self, address: u64, len: usize) -> Result<()>;
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>>;
    fn write(&mut self, address: u64, data: &[u8]) -> Result<()>;

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

/// Memory backed by buffers, for tests.
#[cfg(test)]
pub struct Fake {
    regions: Vec<(u64, Vec<u8>)>,
}

#[cfg(test)]
impl Fake {
    pub fn new(base: u64, bytes: Vec<u8>) -> Self {
        Self {
            regions: vec![(base, bytes)],
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
            .collect();
        Self { regions }
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
    fn snapshot(&mut self, _: u64, _: usize) -> Result<()> {
        Ok(())
    }

    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
        Ok(self.region(address, len)?.to_vec())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        self.region(address, data.len())?.copy_from_slice(data);
        Ok(())
    }
}
