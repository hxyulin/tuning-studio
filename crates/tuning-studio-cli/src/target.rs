//! SWD access to core 0's memory through its MEM-AP, so reads and writes never halt
//! the core. The session sits behind a `FairMutex` so the GDB server can share it. An aligned 1, 2 or 4-byte value moves in one bus transfer of its width,
//! so the core never sees half of a write, which keeps atomics atomic. 8-byte values
//! take two 32-bit transfers. Attaching and detaching each halt it for about 20 ms while probe-rs
//! clears breakpoints.

use anyhow::{Context, Result, anyhow, bail};
use parking_lot::FairMutex;
use probe_rs::architecture::arm::dp::DpAddress;
use probe_rs::architecture::arm::memory::ArmMemoryInterface;
use probe_rs::architecture::arm::{ApV2Address, FullyQualifiedApAddress};
use probe_rs::probe::DebugProbeSelector;
use probe_rs::probe::list::Lister;
use probe_rs::{Permissions, Session};
use probe_rs_target::{ApAddress, CoreAccessOptions, MemoryRegion};
use std::ops::Range;

use crate::memory::Memory;

pub struct Target {
    pub session: FairMutex<Session>,
    ap: FullyQualifiedApAddress,
    /// Flash, where a bus write has no effect
    nvm: Vec<Range<u64>>,
}

impl Target {
    /// Attach without resetting. `probe` is `VID:PID` or `VID:PID:SERIAL`; `None`
    /// takes the only probe connected.
    pub fn attach(chip: &str, probe: Option<&str>, speed_khz: u32) -> Result<Self> {
        let lister = Lister::new();
        let mut probe = match probe {
            Some(selector) => {
                let selector: DebugProbeSelector = selector.parse()?;
                lister.open(selector)?
            }
            None => match lister.list_all().as_slice() {
                [only] => only.open()?,
                [] => bail!("no debug probe found"),
                probes => bail!("{} probes connected; pick one with --probe", probes.len()),
            },
        };
        probe.set_speed(speed_khz)?;
        let session = probe
            .attach(chip, Permissions::default())
            .with_context(|| format!("could not attach to {chip}"))?;
        let ap = core_memory_ap(&session)?;
        let nvm = session
            .target()
            .memory_map
            .iter()
            .filter(|region| matches!(region, MemoryRegion::Nvm(_)))
            .map(|region| region.address_range())
            .collect();
        Ok(Self {
            session: FairMutex::new(session),
            ap,
            nvm,
        })
    }

    /// Memory access through `session`, which must be this target's locked session.
    pub fn memory<'s>(&self, session: &'s mut Session) -> Result<ApMemory<'s>> {
        let ap = session
            .get_arm_interface()?
            .memory_interface(&self.ap)
            .context("could not open the memory AP")?;
        Ok(ApMemory {
            ap,
            nvm: self.nvm.clone(),
            snapshot: None,
        })
    }
}

fn core_memory_ap(session: &Session) -> Result<FullyQualifiedApAddress> {
    let core = session
        .target()
        .cores
        .first()
        .ok_or_else(|| anyhow!("target has no cores"))?;
    let CoreAccessOptions::Arm(options) = &core.core_access_options else {
        bail!("only Arm Cortex-M targets are supported");
    };
    let dp = options
        .targetsel
        .map_or(DpAddress::Default, DpAddress::Multidrop);
    Ok(match &options.ap {
        ApAddress::V1(ap) => FullyQualifiedApAddress::v1_with_dp(dp, *ap),
        ApAddress::V2(ap) => FullyQualifiedApAddress::v2_with_dp(dp, ApV2Address::new(*ap)),
    })
}

pub struct ApMemory<'a> {
    ap: Box<dyn ArmMemoryInterface + 'a>,
    /// Flash, where a bus write has no effect
    nvm: Vec<Range<u64>>,
    /// Bytes read in one transfer, so a struct's fields come from the same moment
    snapshot: Option<(u64, Vec<u8>)>,
}

impl Memory for ApMemory<'_> {
    fn snapshot(&mut self, address: u64, len: usize) -> Result<()> {
        self.snapshot = None;
        let bytes = self.read(address, len)?;
        self.snapshot = Some((address, bytes));
        Ok(())
    }

    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
        if let Some((start, bytes)) = &self.snapshot {
            let offset = address.wrapping_sub(*start);
            if address >= *start && offset + len as u64 <= bytes.len() as u64 {
                return Ok(bytes[offset as usize..][..len].to_vec());
            }
        }
        let mut read = || -> Result<Vec<u8>, probe_rs::Error> {
            Ok(match (len, address % len.max(1) as u64) {
                (1, _) => vec![self.ap.read_word_8(address)?],
                (2, 0) => self.ap.read_word_16(address)?.to_le_bytes().to_vec(),
                (4, 0) => self.ap.read_word_32(address)?.to_le_bytes().to_vec(),
                (8, 0) => self.ap.read_word_64(address)?.to_le_bytes().to_vec(),
                _ => {
                    let mut buf = vec![0; len];
                    self.ap.read(address, &mut buf)?;
                    buf
                }
            })
        };
        read().with_context(|| format!("could not read {len} bytes at {address:#010x}"))
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        let end = address + data.len() as u64;
        if self.nvm.iter().any(|r| address < r.end && r.start < end) {
            bail!("{address:#010x} is in flash, which ignores writes");
        }
        self.snapshot = None;
        let mut write = || -> Result<(), probe_rs::Error> {
            match (data.len(), address % data.len().max(1) as u64) {
                (1, _) => self.ap.write_word_8(address, data[0]),
                (2, 0) => self
                    .ap
                    .write_word_16(address, u16::from_le_bytes(data.try_into().unwrap())),
                (4, 0) => self
                    .ap
                    .write_word_32(address, u32::from_le_bytes(data.try_into().unwrap())),
                (8, 0) => self
                    .ap
                    .write_word_64(address, u64::from_le_bytes(data.try_into().unwrap())),
                _ => self.ap.write(address, data),
            }?;
            Ok(self.ap.flush()?)
        };
        write()
            .with_context(|| format!("could not write {} bytes at {address:#010x}", data.len()))?;
        Ok(())
    }
}
