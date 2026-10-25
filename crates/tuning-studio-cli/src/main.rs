mod memory;
mod path;
mod target;
mod value;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfInfo, ElfParser, Step};

use memory::Memory;
use path::Location;

#[derive(Parser)]
#[command(
    name = "tuning-studio",
    version,
    about = "Inspect and tune a running target over SWD"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read a variable or an address once
    Read {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol path, such as `CONTROL.yaw.target_rpm` or `buffer[3]`, or an address
        /// with an optional type and path, such as `0x24000070`, `0x24000070:f32` or
        /// `0x20000038:Axis.gains.2`. Types other than the primitives need `--elf`.
        location: String,
    },
    /// Write one value, then read it back
    Set {
        #[command(flatten)]
        target: TargetArgs,
        /// Symbol path or address, as for `read`
        location: String,
        #[arg(allow_negative_numbers = true)]
        value: String,
    },
}

#[derive(Args)]
struct TargetArgs {
    /// Firmware ELF with DWARF debug information, needed for symbol names
    #[arg(long)]
    elf: Option<PathBuf>,
    /// probe-rs chip name, such as `STM32H723VGTx`
    #[arg(long)]
    chip: String,
    /// Probe as `VID:PID` or `VID:PID:SERIAL`, needed when several are connected
    #[arg(long)]
    probe: Option<String>,
    /// SWD clock in kHz
    #[arg(long, default_value_t = 4000)]
    speed: u32,
}

impl TargetArgs {
    fn elf(&self) -> Result<Option<ElfInfo>> {
        self.elf
            .as_ref()
            .map(|path| {
                ElfParser::parse(path)
                    .with_context(|| format!("could not parse {}", path.display()))
            })
            .transpose()
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Read { target, location } => {
            let elf = target.elf()?;
            let mut probe =
                target::Target::attach(&target.chip, target.probe.as_deref(), target.speed)?;
            let mut memory = probe.memory()?;
            let location = path::locate(elf.as_ref(), &mut memory, &location)?;
            print!("{}", read(elf.as_ref(), &mut memory, &location)?);
        }
        Command::Set {
            target,
            location,
            value,
        } => {
            let elf = target.elf()?;
            let mut probe =
                target::Target::attach(&target.chip, target.probe.as_deref(), target.speed)?;
            let mut memory = probe.memory()?;
            let location = path::locate(elf.as_ref(), &mut memory, &location)?;
            set(elf.as_ref(), &mut memory, &location, &value)?;
            print!("{}", read(elf.as_ref(), &mut memory, &location)?);
        }
    }
    Ok(())
}

fn read(elf: Option<&ElfInfo>, memory: &mut dyn Memory, location: &Location) -> Result<String> {
    match (location, elf) {
        (Location::Node(node), Some(elf)) => value::render(elf, memory, node),
        (&Location::Raw { address, scalar }, _) => {
            let bytes = memory.read(address, value::size_of(scalar))?;
            Ok(format!(
                "{address:#010x}: {scalar} = {}\n",
                value::decode(scalar, &bytes)
            ))
        }
        (Location::Node(_), None) => unreachable!("symbol paths need an ELF"),
    }
}

fn set(
    elf: Option<&ElfInfo>,
    memory: &mut dyn Memory,
    location: &Location,
    text: &str,
) -> Result<()> {
    let (address, bytes) = match location {
        &Location::Raw { address, scalar } => (address, value::encode(scalar, text)?),
        Location::Node(node) => {
            let elf = elf.expect("symbol paths need an ELF");
            let path = &node.path;
            if !node.readable {
                bail!(
                    "{path} is not readable: {}",
                    node.status.as_deref().unwrap_or("unknown")
                );
            }
            if node.bit_size.is_some() {
                bail!("{path} is a bitfield; writing bitfields is not supported yet");
            }
            let (NodeKind::Scalar | NodeKind::Enum, Some(scalar)) =
                (node.kind, value::scalar_of(node))
            else {
                bail!(
                    "{path} is a {}; set one of its fields instead",
                    node.type_name
                );
            };
            // Behind a pointer the target section is unknown, so only direct fields are checked.
            let direct = !node
                .node
                .steps
                .iter()
                .any(|s| matches!(s, Step::Deref | Step::SliceIndex(_)));
            if direct
                && elf
                    .find_symbol(&node.node.symbol)
                    .is_some_and(|s| !s.writable)
            {
                bail!("{path} is in a read-only section");
            }
            let bytes = if node.kind == NodeKind::Enum {
                let variants = tree::enum_variants(elf, &node.node, &mut |address, len| {
                    memory.word(address, len).map_err(|e| format!("{e:#}"))
                })
                .map_err(|e| anyhow!("{e}"))?
                .unwrap_or_default();
                value::encode_enum(&variants, value::size_of(scalar), text)?
            } else if node.type_name == "char" {
                value::encode_char(text)?
            } else {
                value::encode(scalar, text)?
            };
            (node.address, bytes)
        }
    };
    memory.write(address, &bytes)?;
    if memory.read(address, bytes.len())? != bytes {
        eprintln!(
            "warning: the value read back differs from the value written. The firmware may \
             have changed it since, or this memory ignores writes."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::Fake;

    #[test]
    fn sets_scalars_only() {
        let elf = ElfParser::parse(format!(
            "{}/../studio-dwarf/tests/fixtures/test_struct.elf",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let symbol = elf.find_symbol("device_config").unwrap();
        let mut memory = Fake::new(symbol.address, vec![0; symbol.size as usize]);
        let location = path::locate(Some(&elf), &mut memory, "device_config.sensor.value").unwrap();
        set(Some(&elf), &mut memory, &location, "3.25").unwrap();
        assert_eq!(
            read(Some(&elf), &mut memory, &location).unwrap(),
            "device_config.sensor.value: float = 3.25\n"
        );
        let location = path::locate(Some(&elf), &mut memory, "device_config.enabled").unwrap();
        assert!(set(Some(&elf), &mut memory, &location, "256").is_err());
        let location = path::locate(Some(&elf), &mut memory, "device_config.sensor").unwrap();
        assert!(set(Some(&elf), &mut memory, &location, "1").is_err());
    }

    #[test]
    fn sets_lab_enums_chars_and_atomics() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/type_lab.elf");
        let elf = ElfParser::parse(path).unwrap();
        let mut memory = Fake::from_elf(path);
        for (location, value, expected) in [
            ("enums::DIRECTION", "Forward", "Direction = Forward (1)"),
            ("enums::DIRECTION", "-1", "Direction = Reverse (-1)"),
            ("scalars::CHAR", "😀", "char = '😀' (U+1F600)"),
            ("atomics::BOOL", "false", "Atomic<bool> = false"),
            ("atomics::I16", "-2", "Atomic<i16> = -2"),
            ("ROBOT.id.1", "-100", "i16 = -100"),
        ] {
            let location = path::locate(Some(&elf), &mut memory, location).unwrap();
            set(Some(&elf), &mut memory, &location, value).unwrap();
            let text = read(Some(&elf), &mut memory, &location).unwrap();
            assert!(text.ends_with(&format!(": {expected}\n")), "{text}");
        }
        for (location, value) in [
            ("enums::DIRECTION", "5"),
            ("enums::MODE", "Sideways"),
            ("atomics::BOOL", "2"),
            ("atomics::PTR", "0"),
            ("TABLE[0]", "9"),
        ] {
            let location = path::locate(Some(&elf), &mut memory, location).unwrap();
            assert!(set(Some(&elf), &mut memory, &location, value).is_err());
        }
    }
}
