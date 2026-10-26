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

use crate::daemon;
use crate::memory::{Memory, Snapshots};

pub struct Target {
    pub session: FairMutex<Session>,
    /// The probe, as `daemon::probe_id` names it
    pub probe_id: String,
    ap: FullyQualifiedApAddress,
    /// Flash, where a bus write has no effect
    nvm: Vec<Range<u64>>,
    ram: Vec<Range<u64>>,
}

impl Target {
    /// Attach without resetting. `probe` is `VID:PID` or `VID:PID:SERIAL`; `None`
    /// takes the only probe connected. `permissions` can allow a full chip erase,
    /// which some chips need to unlock.
    pub fn attach(
        chip: &str,
        probe: Option<&str>,
        speed_khz: u32,
        permissions: Permissions,
    ) -> Result<Self> {
        let probes = Lister::new().list_all();
        let info = match probe {
            Some(text) => {
                let selector: DebugProbeSelector = text.parse()?;
                probes
                    .into_iter()
                    .find(|info| selector.matches_probe(info))
                    .ok_or_else(|| anyhow!("no probe {text} is connected"))?
            }
            None => match <[_; 1]>::try_from(probes) {
                Ok([only]) => only,
                Err(probes) if probes.is_empty() => bail!("no debug probe found"),
                Err(probes) => {
                    bail!("{} probes connected; pick one with --probe", probes.len())
                }
            },
        };
        let probe_id = daemon::probe_id(
            info.vendor_id,
            info.product_id,
            info.serial_number.as_deref(),
        );
        let mut probe = info.open()?;
        probe.set_speed(speed_khz)?;
        let session = probe
            .attach(chip, permissions)
            .with_context(|| format!("could not attach to {chip}"))?;
        let ap = core_memory_ap(&session)?;
        let regions = |ram: bool| {
            session
                .target()
                .memory_map
                .iter()
                .filter(|region| match region {
                    MemoryRegion::Nvm(_) => !ram,
                    MemoryRegion::Ram(_) => ram,
                    _ => false,
                })
                .map(|region| region.address_range())
                .collect()
        };
        let (nvm, ram) = (regions(false), regions(true));
        Ok(Self {
            session: FairMutex::new(session),
            probe_id,
            ap,
            nvm,
            ram,
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
            ram: self.ram.clone(),
            snapshots: Snapshots::default(),
        })
    }
}

/// Print each connected probe with the selector `--probe` takes.
pub fn list() {
    let probes = Lister::new().list_all();
    if probes.is_empty() {
        println!("no debug probes found");
    }
    for probe in probes {
        let serial = probe
            .serial_number
            .as_deref()
            .map(|serial| format!(":{serial}"))
            .unwrap_or_default();
        println!(
            "{:04x}:{:04x}{serial}  {}",
            probe.vendor_id, probe.product_id, probe.identifier
        );
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
    ram: Vec<Range<u64>>,
    snapshots: Snapshots,
}

impl Memory for ApMemory<'_> {
    fn transfer(&mut self, address: u64, len: usize) -> Result<Vec<u8>> {
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
        let end = address
            .checked_add(data.len() as u64)
            .context("the write runs past the end of the address space")?;
        if self.nvm.iter().any(|r| address < r.end && r.start < end) {
            bail!("{address:#010x} is in flash, which ignores writes");
        }
        self.forget();
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

    fn snapshots(&mut self) -> &mut Snapshots {
        &mut self.snapshots
    }

    fn ram(&self) -> &[Range<u64>] {
        &self.ram
    }
}
