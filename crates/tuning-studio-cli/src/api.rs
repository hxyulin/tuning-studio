//! Values the firmware declares with `tuning-studio-api`: `Watch` statics, which
//! the host only reads, and `Param`s, which it changes through the firmware.
//! Both are found by type name and read by field name.

use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfInfo, NodeRef, Step, SymbolNode, VariableType};

use crate::memory::{self, Memory};
use crate::path;
use crate::value;

/// Reads of a `Watch` before giving up on one the firmware never stops writing
const WATCH_ATTEMPTS: usize = 10;
/// How long a `set` waits for the firmware to answer
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);

// `reply` values from tuning-studio-api
const APPLIED: u64 = 1;
const CLAMPED: u64 = 2;
const REFUSED: u64 = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Watch,
    Param,
}

pub fn kind(node: &SymbolNode) -> Option<Kind> {
    if node.kind != NodeKind::Struct {
        return None;
    }
    if node.type_name.starts_with("Watch<") {
        Some(Kind::Watch)
    } else if node.type_name.starts_with("Param<") {
        Some(Kind::Param)
    } else {
        None
    }
}

/// The `Watch` or `Param` that `node` is a field of, if any.
pub fn owner(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<Option<(Kind, SymbolNode)>> {
    for end in 0..node.node.steps.len() {
        let prefix = NodeRef {
            steps: node.node.steps[..end].to_vec(),
            ..node.node.clone()
        };
        let parent = path::describe(elf, memory, &prefix)?;
        if let Some(kind) = kind(&parent) {
            return Ok(Some((kind, parent)));
        }
    }
    Ok(None)
}

fn field(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
    name: &str,
) -> Result<SymbolNode> {
    path::describe(elf, memory, &node.node.child(Step::Member(name.into()))).map_err(|e| {
        anyhow!(
            "{} has no field `{name}` ({e}); is it from tuning-studio-api 0.2?",
            node.path
        )
    })
}

fn scalar(node: &SymbolNode) -> Result<VariableType> {
    value::scalar_of(node)
        .ok_or_else(|| anyhow!("{} is a {}, not a number", node.path, node.type_name))
}

fn text(memory: &mut dyn Memory, node: &SymbolNode) -> Result<String> {
    let scalar = scalar(node)?;
    Ok(value::decode(
        scalar,
        &memory.read(node.address, value::size_of(scalar))?,
    ))
}

fn field_text(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
    name: &str,
) -> Result<String> {
    let field = field(elf, memory, node, name)?;
    text(memory, &field)
}

/// Unit strings by the address of the `unit` field that points to them. They
/// live in flash, so each is read once, and later reads of the value cost no
/// extra probe round trip.
static UNITS: Mutex<Option<HashMap<u64, String>>> = Mutex::new(None);

/// Forget the cached units, after flashing firmware whose strings may differ.
pub fn forget_units() {
    *UNITS.lock().unwrap() = None;
}

/// The text of `node`'s `unit` field with a space before it, or nothing when it
/// is empty or the firmware's tuning-studio-api has no unit there.
fn unit(elf: &ElfInfo, memory: &mut dyn Memory, node: &SymbolNode) -> String {
    let Ok(field) = field(elf, memory, node, "unit") else {
        return String::new();
    };
    if let Some(unit) = UNITS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|units| units.get(&field.address))
    {
        return unit.clone();
    }
    let Ok(Some(preview)) = tree::preview(elf, &field.node, &mut |address, len| {
        memory.read(address, len).map_err(|e| format!("{e:#}"))
    }) else {
        return String::new();
    };
    let unit = match preview.text.trim_matches('"') {
        "" => String::new(),
        unit => format!(" {unit}"),
    };
    UNITS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(field.address, unit.clone());
    unit
}

/// The bytes [`render_watch`] reads: only the counters and what lies between
/// them, so the window a write can overlap stays as short as it can be.
pub fn watch_span(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<(u64, usize)> {
    let end = field(elf, memory, node, "end")?;
    let start = field(elf, memory, node, "start")?;
    Ok((end.address, (start.address + 4 - end.address) as usize))
}

/// Print a `Watch`'s value as its name, if the firmware has written it. The
/// counters around the value must match in the same read, or the read saw a
/// write in progress and is retried.
pub fn render_watch(elf: &ElfInfo, memory: &mut dyn Memory, node: &SymbolNode) -> Result<String> {
    let end = field(elf, memory, node, "end")?;
    let start = field(elf, memory, node, "start")?;
    let mut value = field(elf, memory, node, "value")?;
    value.wrapper = None;
    let (_, size) = watch_span(elf, memory, node)?;
    let unit = unit(elf, memory, node);
    for attempt in 0..WATCH_ATTEMPTS {
        if attempt == 0 {
            memory.snapshot(end.address, size)?;
        } else {
            memory.reread(end.address, size)?;
        }
        let (ends, starts) = (memory.word(end.address, 4)?, memory.word(start.address, 4)?);
        if starts == 0 {
            return Ok(format!(
                "{}: {} = <not written yet>\n",
                node.path, value.type_name
            ));
        }
        if ends == starts {
            let mut text = value::render_snapshot(elf, memory, &value, &node.path);
            // The unit follows a single value; a struct's fields have none yet.
            if value.scalar.is_some() && !unit.is_empty() {
                text.insert_str(text.len() - 1, &unit);
            }
            return Ok(text);
        }
    }
    // Inline, as other unreadable values are, so a `watch` keeps going.
    Ok(format!(
        "{}: {} = <changed during each of {WATCH_ATTEMPTS} reads>\n",
        node.path, value.type_name
    ))
}

/// The field holding the value of a `Watch` or `Param`, and the unit shown after
/// it.
pub fn traced_value(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
) -> Result<(SymbolNode, String)> {
    let mut value = field(elf, memory, node, "value")?;
    value.wrapper = None;
    Ok((value, unit(elf, memory, node)))
}

/// The fields of a `Param` the host shows
struct Param {
    value: SymbolNode,
    target: SymbolNode,
    request: SymbolNode,
    request_id: SymbolNode,
    reply_id: SymbolNode,
    reply: SymbolNode,
}

impl Param {
    fn new(elf: &ElfInfo, memory: &mut dyn Memory, node: &SymbolNode) -> Result<Self> {
        let mut get = |name| field(elf, memory, node, name);
        Ok(Self {
            value: get("value")?,
            target: get("target")?,
            request: get("request")?,
            request_id: get("request_id")?,
            reply_id: get("reply_id")?,
            reply: get("reply")?,
        })
    }
}

/// One line: the value, then what it moves toward and its limits.
///
/// ```text
/// tune_lab::SETPOINT: f32 = 50.0 rad/s (target 100.0, default 100.0, -300.0 to 300.0, step 1.0)
/// ```
pub fn render_param(elf: &ElfInfo, memory: &mut dyn Memory, node: &SymbolNode) -> Result<String> {
    memory.snapshot(node.address, param_size(node)?)?;
    let param = Param::new(elf, memory, node)?;
    let scalar = scalar(&param.value)?;
    let value = text(memory, &param.value)?;
    let target = text(memory, &param.target)?;
    let mut notes = Vec::new();
    if target != value {
        notes.push(format!("target {target}"));
    }
    notes.push(format!(
        "default {}",
        field_text(elf, memory, node, "default")?
    ));
    if scalar != VariableType::Bool {
        let min = field_text(elf, memory, node, "min")?;
        let max = field_text(elf, memory, node, "max")?;
        let (lowest, highest) = extremes(scalar);
        if min != lowest || max != highest {
            notes.push(format!("{min} to {max}"));
        }
        let step = field_text(elf, memory, node, "max_step")?;
        if step != highest {
            notes.push(format!("step {step}"));
        }
    }
    if memory.word(param.request_id.address, 4)? != memory.word(param.reply_id.address, 4)? {
        notes.push("a request is waiting for the firmware".into());
    }
    let unit = unit(elf, memory, node);
    Ok(format!(
        "{}: {} = {value}{unit} ({})\n",
        node.path,
        param.value.type_name,
        notes.join(", ")
    ))
}

/// The text of a type's lowest and highest values, which mean "no limit".
fn extremes(scalar: VariableType) -> (String, String) {
    let pair = |min: i128, max: i128| (min.to_string(), max.to_string());
    match scalar {
        VariableType::I8 => pair(i8::MIN.into(), i8::MAX.into()),
        VariableType::I16 => pair(i16::MIN.into(), i16::MAX.into()),
        VariableType::I32 => pair(i32::MIN.into(), i32::MAX.into()),
        VariableType::U8 => pair(0, u8::MAX.into()),
        VariableType::U16 => pair(0, u16::MAX.into()),
        VariableType::U32 => pair(0, u32::MAX.into()),
        _ => ("-inf".into(), "inf".into()),
    }
}

/// Ask the firmware to change a `Param`, and wait for its answer. Returns a note
/// when the firmware changed the request.
pub fn request(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    node: &SymbolNode,
    text: &str,
) -> Result<String> {
    let param = Param::new(elf, memory, node)?;
    let scalar = scalar(&param.value)?;
    let mut bytes = value::encode(scalar, text)?;
    bytes.resize(4, 0);
    let id = (memory.word(param.request_id.address, 4)? as u32).wrapping_add(1);
    // The firmware reads `request` once it sees the new id, so the id goes last.
    memory.write(param.request.address, &bytes)?;
    memory.write(param.request_id.address, &id.to_le_bytes())?;
    let start = Instant::now();
    while memory.word(param.reply_id.address, 4)? != u64::from(id) {
        if start.elapsed() > REPLY_TIMEOUT {
            bail!(
                "the firmware did not answer within {} ms. It applies requests in \
                 `Param::get`; is the core halted, or is `{}` never read? The request \
                 stays waiting.",
                REPLY_TIMEOUT.as_millis(),
                node.path
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    match memory.word(param.reply.address, 1)? {
        APPLIED => Ok(String::new()),
        CLAMPED => Ok(format!(
            "{text} is outside the range; the firmware clamped it to {}\n",
            self::text(memory, &param.target)?
        )),
        REFUSED => bail!("the firmware refused {text} for {}", node.path),
        other => bail!("the firmware answered with an unknown reply {other}"),
    }
}

/// Every `Param`, then every `Watch`, in the ELF.
pub fn values(elf: &ElfInfo, memory: &mut dyn Memory) -> Result<String> {
    let mut found: Vec<(Kind, SymbolNode)> = Vec::new();
    for symbol in elf.get_variables() {
        let Ok(node) = path::describe(elf, memory, &NodeRef::root(symbol.demangled_name.clone()))
        else {
            continue;
        };
        if let Some(kind) = kind(&node) {
            found.push((kind, node));
        }
    }
    found.sort_by(|a, b| (a.0 == Kind::Watch, &a.1.path).cmp(&(b.0 == Kind::Watch, &b.1.path)));
    if found.is_empty() {
        bail!("the ELF declares no `Param` or `Watch` from tuning-studio-api");
    }
    let spans = found
        .iter()
        .map(|(kind, node)| span(elf, memory, *kind, node))
        .collect::<Result<Vec<_>>>()?;
    memory::prefetch(memory, spans)?;
    let mut out = String::new();
    for (kind, node) in &found {
        out += &match kind {
            Kind::Param => render_param(elf, memory, node)?,
            Kind::Watch => render_watch(elf, memory, node)?,
        };
    }
    memory.forget();
    Ok(out)
}

/// The bytes rendering `node` reads at once: a `Watch`'s counters and value, or a
/// whole `Param`.
pub fn span(
    elf: &ElfInfo,
    memory: &mut dyn Memory,
    kind: Kind,
    node: &SymbolNode,
) -> Result<(u64, usize)> {
    match kind {
        Kind::Watch => watch_span(elf, memory, node),
        Kind::Param => Ok((node.address, param_size(node)?)),
    }
}

fn param_size(node: &SymbolNode) -> Result<usize> {
    Ok(node
        .size
        .ok_or_else(|| anyhow!("{} has no size", node.path))? as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Fake;
    use crate::path::Location;
    use studio_dwarf::ElfParser;

    const ELF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tune_lab.elf");

    fn node(elf: &ElfInfo, memory: &mut Fake, text: &str) -> SymbolNode {
        match path::locate(Some(elf), memory, text).unwrap() {
            Location::Node(node) => *node,
            Location::Raw { .. } => panic!("{text} is not a symbol"),
        }
    }

    #[test]
    fn reads_watches_only_when_the_counters_match() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let speed = node(&elf, &mut memory, "motor.speed");
        assert_eq!(kind(&speed), Some(Kind::Watch));
        assert_eq!(
            render_watch(&elf, &mut memory, &speed).unwrap(),
            "motor.speed: f32 = <not written yet>\n"
        );
        // end, value, start
        let write = |memory: &mut Fake, end: u32, value: f32, start: u32| {
            memory.write(speed.address, &end.to_le_bytes()).unwrap();
            memory
                .write(speed.address + 4, &value.to_le_bytes())
                .unwrap();
            memory
                .write(speed.address + 8, &start.to_le_bytes())
                .unwrap();
        };
        write(&mut memory, 7, 12.5, 7);
        assert_eq!(
            render_watch(&elf, &mut memory, &speed).unwrap(),
            "motor.speed: f32 = 12.5 rad/s\n"
        );
        write(&mut memory, 7, 12.5, 8);
        assert_eq!(
            render_watch(&elf, &mut memory, &speed).unwrap(),
            "motor.speed: f32 = <changed during each of 10 reads>\n"
        );
        // A watch without a unit shows none.
        let output = node(&elf, &mut memory, "motor.output");
        for name in ["end", "start"] {
            let counter = field(&elf, &mut memory, &output, name).unwrap();
            memory.write(counter.address, &1u32.to_le_bytes()).unwrap();
        }
        assert_eq!(
            render_watch(&elf, &mut memory, &output).unwrap(),
            "motor.output: f32 = 0.0\n"
        );

        let state = node(&elf, &mut memory, "motor.state");
        // `Motor` holds a u64, so padding follows `end`.
        for name in ["end", "start"] {
            let counter = field(&elf, &mut memory, &state, name).unwrap();
            memory.write(counter.address, &3u32.to_le_bytes()).unwrap();
        }
        // A derived snapshot: `pi` nests, and the skipped `driver` is absent.
        assert_eq!(
            render_watch(&elf, &mut memory, &state).unwrap(),
            "motor.state: MotorSnapshot\n  speed: f32 = 0.0\n  ticks: u64 = 0\n  pi: PiSnapshot\n    \
             integral: f32 = 0.0\n    saturated: bool = false\n"
        );
    }

    #[test]
    fn renders_params_with_their_limits() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        for (path, expected) in [
            (
                "tune_lab::SETPOINT",
                "f32 = 100.0 rad/s (default 100.0, -300.0 to 300.0, step 1.0)",
            ),
            (
                "tune_lab::KI",
                "f32 = 0.5 (default 0.5, 0.0 to 10.0, step 0.1)",
            ),
            ("tune_lab::ENABLE", "bool = true (default true)"),
            (
                "tune_lab::OUTPUT_LIMIT",
                "u8 = 100 % (default 100, 0 to 100, step 5)",
            ),
        ] {
            let param = node(&elf, &mut memory, path);
            assert_eq!(
                render_param(&elf, &mut memory, &param).unwrap(),
                format!("{path}: {expected}\n")
            );
        }
    }

    #[test]
    fn requests_wait_for_the_firmware() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let kp = node(&elf, &mut memory, "tune_lab::KP");
        // No firmware runs here, so nothing answers.
        let error = request(&elf, &mut memory, &kp, "0.05")
            .unwrap_err()
            .to_string();
        assert!(error.contains("did not answer"), "{error}");
        assert_eq!(
            memory.word(kp.address + 8, 4).unwrap(),
            u64::from(0.05f32.to_bits())
        );
        assert_eq!(memory.word(kp.address + 12, 4).unwrap(), 1);
        let text = render_param(&elf, &mut memory, &kp).unwrap();
        assert!(
            text.ends_with("a request is waiting for the firmware)\n"),
            "{text}"
        );
    }

    #[test]
    fn lists_params_then_watches() {
        let elf = ElfParser::parse(ELF).unwrap();
        let mut memory = Fake::from_elf(ELF);
        let names: Vec<String> = values(&elf, &mut memory)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with(' '))
            .map(|line| line.split(':').next().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "tune_lab",
                "tune_lab",
                "tune_lab",
                "tune_lab",
                "tune_lab",
                "motor.error",
                "motor.output",
                "motor.speed",
                "motor.state"
            ]
        );
    }
}
