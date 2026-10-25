//! Debug probe carrier over probe-rs: SWD memory access through core 0's MEM-AP.
//!
//! Memory goes straight through the access port rather than a probe-rs `Core`:
//! sampling never halts the core, and the AP handle is kept open for as long as
//! the session samples, which saves several USB round trips per read.
//!
//! Opening and dropping the probe-rs `Session` each clear the hardware
//! breakpoints with every core halted: about 20 ms per halt on an STM32H723
//! over CMSIS-DAP (`examples/attach_halt.rs`).

use probe_rs::architecture::arm::dp::DpAddress;
use probe_rs::architecture::arm::memory::ArmMemoryInterface;
use probe_rs::architecture::arm::{ApV2Address, FullyQualifiedApAddress};
use probe_rs::config::Registry;
use probe_rs::probe::list::Lister;
use probe_rs::probe::DebugProbeSelector;
use probe_rs::{Permissions, Session};
use probe_rs_target::{ApAddress, CoreAccessOptions};
use serde::{Deserialize, Serialize};

use crate::{CarrierError, Link, MemoryAccess, Result};

/// SWD clock when the user does not pick one; probes clamp it to what they support.
pub const DEFAULT_SPEED_KHZ: u32 = 4000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    /// Pass back to [`ProbeLink::open`]; `VID:PID` or `VID:PID:SERIAL`
    pub selector: String,
    pub name: String,
    pub serial: Option<String>,
}

/// Call once from the main thread before any probe is listed or opened.
///
/// On macOS hidapi's first init schedules its global HID manager on the calling
/// thread's run loop, and every later enumeration schedules each matched device on
/// that same run loop. Done first on a worker thread, the run loop dies with the
/// thread and the next probe open crashes in `CFRunLoopAddSource`; the main
/// thread's run loop lives as long as the app.
pub fn init_hid_on_main_thread() {
    #[cfg(target_os = "macos")]
    if let Err(e) = hidapi::HidApi::new() {
        tracing::warn!("hidapi init failed; CMSIS-DAP v1 probes unavailable: {e}");
    }
}

pub fn list_probes() -> Vec<ProbeInfo> {
    Lister::new()
        .list_all()
        .iter()
        .map(|p| ProbeInfo {
            selector: DebugProbeSelector::from(p).to_string(),
            name: p.identifier.clone(),
            serial: p.serial_number.clone(),
        })
        .collect()
}

/// Chip names probe-rs knows, containing `query` (case-insensitive), at most `limit`.
pub fn search_chips(query: &str, limit: usize) -> Vec<String> {
    let needle = query.to_ascii_lowercase();
    let registry = Registry::from_builtin_families();
    let mut names: Vec<String> = registry
        .families()
        .iter()
        .flat_map(|f| f.variants.iter().map(|c| c.name.clone()))
        .filter(|name| name.to_ascii_lowercase().contains(&needle))
        .collect();
    names.sort();
    names.truncate(limit);
    names
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeConfig {
    /// From [`ProbeInfo::selector`]; `None` takes the only probe attached
    pub selector: Option<String>,
    /// probe-rs chip name, e.g. `STM32H723VG`
    pub chip: String,
    /// `None` uses [`DEFAULT_SPEED_KHZ`]
    pub speed_khz: Option<u32>,
}

pub struct ProbeLink {
    session: Session,
    /// The MEM-AP core 0 is debugged through; it sees the core's memory map
    ap: FullyQualifiedApAddress,
}

impl ProbeLink {
    /// Attach without resetting; the core halts briefly while probe-rs clears breakpoints.
    pub fn open(config: &ProbeConfig) -> Result<Self> {
        let lister = Lister::new();
        let mut probe = match &config.selector {
            Some(selector) => {
                let parsed: DebugProbeSelector = selector
                    .parse()
                    .map_err(|e| CarrierError::Probe(format!("{e}")))?;
                lister.open(parsed).map_err(|e| match e {
                    probe_rs::probe::DebugProbeError::ProbeCouldNotBeCreated(_) => {
                        CarrierError::ProbeNotFound(selector.clone())
                    }
                    other => CarrierError::Probe(other.to_string()),
                })?
            }
            None => {
                let probes = lister.list_all();
                match probes.as_slice() {
                    [only] => only
                        .open()
                        .map_err(|e| CarrierError::Probe(e.to_string()))?,
                    [] => return Err(CarrierError::ProbeNotFound("any probe".into())),
                    _ => {
                        return Err(CarrierError::Probe(format!(
                            "{} probes attached; pick one",
                            probes.len()
                        )))
                    }
                }
            }
        };
        probe
            .set_speed(config.speed_khz.unwrap_or(DEFAULT_SPEED_KHZ))
            .map_err(|e| CarrierError::Probe(e.to_string()))?;
        let session = probe
            .attach(config.chip.as_str(), Permissions::default())
            .map_err(|e| CarrierError::Attach(e.to_string()))?;
        let ap = core_memory_ap(&session)?;
        tracing::info!(?ap, "attached");
        Ok(Self { session, ap })
    }

    /// Flash only when explicitly requested, then reset and run core 0.
    /// Call before starting acquisition, while this process owns the probe exclusively.
    pub fn prepare(config: &ProbeConfig, elf: &std::path::Path, flash: bool) -> Result<()> {
        let mut link = Self::open(config)?;
        if flash {
            let mut options = probe_rs::flashing::DownloadOptions::default();
            options.verify = true;
            probe_rs::flashing::download_file_with_options(
                &mut link.session,
                elf,
                probe_rs::flashing::ElfLoader(Default::default()),
                options,
            )
            .map_err(|e| CarrierError::Other(format!("flash failed: {e}")))?;
        }
        link.session
            .core(0)
            .and_then(|mut core| core.reset())
            .map_err(|e| CarrierError::Other(format!("reset failed: {e}")))
    }
}

fn core_memory_ap(session: &Session) -> Result<FullyQualifiedApAddress> {
    let core = session
        .target()
        .cores
        .first()
        .ok_or_else(|| CarrierError::Attach("target has no cores".into()))?;
    let CoreAccessOptions::Arm(options) = &core.core_access_options else {
        return Err(CarrierError::Attach(
            "only Arm Cortex-M targets are supported".into(),
        ));
    };
    let dp = options
        .targetsel
        .map_or(DpAddress::Default, DpAddress::Multidrop);
    Ok(match &options.ap {
        ApAddress::V1(ap) => FullyQualifiedApAddress::v1_with_dp(dp, *ap),
        ApAddress::V2(ap) => FullyQualifiedApAddress::v2_with_dp(dp, ApV2Address::new(*ap)),
    })
}

impl Link for ProbeLink {
    fn with_memory(&mut self, body: &mut dyn FnMut(&mut dyn MemoryAccess)) -> Result<()> {
        let interface = self
            .session
            .get_arm_interface()
            .map_err(|e| CarrierError::Other(e.to_string()))?;
        let mut memory = interface
            .memory_interface(&self.ap)
            .map_err(|e| CarrierError::Other(format!("could not open the memory AP: {e}")))?;
        body(&mut ApMemory(&mut *memory));
        Ok(())
    }
}

struct ApMemory<'a>(&'a mut dyn ArmMemoryInterface);

impl MemoryAccess for ApMemory<'_> {
    fn read(&mut self, address: u64, buf: &mut [u8]) -> Result<()> {
        let len = buf.len();
        self.0.read(address, buf).map_err(|e| CarrierError::Read {
            address,
            len,
            reason: e.to_string(),
        })
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        self.0
            .write(address, data)
            .map_err(|e| CarrierError::Write {
                address,
                len: data.len(),
                reason: e.to_string(),
            })
    }
}
