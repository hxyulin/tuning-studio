import { render } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import "./style.css";
import * as history from "./history";
import { control, step } from "./sim";
import { type Node, byId, roots } from "./tree";

const SLOTS = 8;
const WINDOWS = [2, 5, 10, 30];

/** Plot range, shared by every lane */
const view = { from: -5, to: 0, paused: false };
const lanes = new Set<() => void>();

function now() {
  return control.tick / 1000;
}

function format(v: number | string) {
  if (typeof v === "string") return v;
  if (Number.isInteger(v)) return String(v);
  const a = Math.abs(v);
  return v.toFixed(a >= 100 ? 1 : a >= 1 ? 3 : 4);
}

interface Trace {
  id: string;
  slot: number;
}

function App() {
  const [connected, setConnected] = useState(true);
  const [paused, setPaused] = useState(false);
  const [span, setSpan] = useState(5);
  const [traces, setTraces] = useState<Trace[]>([
    { id: "control.motors[0].angle_rad", slot: 0 },
    { id: "control.yaw.target_angle_rad", slot: 1 },
    { id: "control.motors[0].speed_rpm", slot: 2 },
    { id: "control.motors[0].target_rpm", slot: 3 },
  ]);
  const [side, setSide] = useState(380);
  const [recording, setRecording] = useState<number | null>(null);
  const [, setFrame] = useState(0);

  // The target: 1 kHz control loop, sampled at 500 Hz
  useEffect(() => {
    if (!connected) return;
    const timer = setInterval(() => {
      for (let i = 0; i < 10; i++) {
        step();
        if (control.tick % 2 === 0) history.record(now());
      }
    }, 10);
    return () => clearInterval(timer);
  }, [connected]);

  // Readouts repaint at 10 Hz; plots every frame
  useEffect(() => {
    const timer = setInterval(() => setFrame((n) => n + 1), 100);
    let raf = 0;
    const draw = () => {
      if (!view.paused) {
        view.to = now();
        view.from = view.to - span;
        lanes.forEach((update) => update());
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => {
      clearInterval(timer);
      cancelAnimationFrame(raf);
    };
  }, [span]);

  view.paused = paused;

  const toggle = (id: string) =>
    setTraces((ts) => {
      if (ts.some((t) => t.id === id)) return ts.filter((t) => t.id !== id);
      const used = new Set(ts.map((t) => t.slot));
      const slot = [...Array(SLOTS).keys()].find((s) => !used.has(s));
      return slot === undefined ? ts : [...ts, { id, slot }];
    });

  const resize = (e: PointerEvent) => {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const start = e.clientX;
    const from = side;
    const move = (m: PointerEvent) => setSide(Math.max(240, Math.min(from + m.clientX - start, innerWidth - 320)));
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  return (
    <div class="app" style={{ "--side": `${side}px` }}>
      <header class="bar">
        <span class="firmware" title="target/thumbv7em-none-eabihf/release/control-training-template">
          control-training-template
        </span>
        <span class="target">
          <span class={`dot ${connected ? "on" : ""}`} />
          {connected ? "STM32H723VGTx" : "Disconnected"}
        </span>
        <span class="grow" />
        <span class="segmented" role="radiogroup" aria-label="Time window">
          {WINDOWS.map((w) => (
            <button role="radio" aria-checked={w === span} onClick={() => setSpan(w)}>
              {w} s
            </button>
          ))}
        </span>
        <button onClick={() => setPaused((p) => !p)} disabled={!connected} aria-pressed={paused}>
          {paused ? "Resume" : "Pause"}
        </button>
        <RecordButton recording={recording} disabled={!connected} onToggle={() => setRecording((r) => (r === null ? now() : null))} />
        <button
          class={connected ? "" : "primary"}
          onClick={() => {
            setConnected((c) => !c);
            setRecording(null);
            setPaused(false);
          }}
        >
          {connected ? "Disconnect" : "Connect"}
        </button>
      </header>
      <main class="split">
        <LiveWatch connected={connected} traces={traces} onToggle={toggle} />
        <div class="divider" role="separator" aria-orientation="vertical" onPointerDown={resize} />
        <Plot traces={traces} paused={paused} />
      </main>
    </div>
  );
}

function RecordButton({ recording, disabled, onToggle }: { recording: number | null; disabled: boolean; onToggle: () => void }) {
  if (recording === null)
    return (
      <button onClick={onToggle} disabled={disabled}>
        Record
      </button>
    );
  const s = Math.floor(now() - recording);
  return (
    <button class="recording" onClick={onToggle}>
      <span class="rec" />
      {`${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`}
    </button>
  );
}

// Live Watch

function matches(node: Node, filter: string): boolean {
  return node.id.toLowerCase().includes(filter) || (node.children?.some((c) => matches(c, filter)) ?? false);
}

function LiveWatch({ connected, traces, onToggle }: { connected: boolean; traces: Trace[]; onToggle: (id: string) => void }) {
  const [filter, setFilter] = useState("");
  const [open, setOpen] = useState(() => new Set(["control", "control.motors", "control.motors[0]", "control.yaw", "control.yaw.speed_pid"]));
  const q = filter.trim().toLowerCase();
  const slots = new Map(traces.map((t) => [t.id, t.slot]));

  const rows: { node: Node; depth: number }[] = [];
  const walk = (nodes: Node[], depth: number) => {
    for (const node of nodes) {
      if (q && !matches(node, q)) continue;
      rows.push({ node, depth });
      if (node.children && (q || open.has(node.id))) walk(node.children, depth + 1);
    }
  };
  walk(roots, 0);

  const flip = (id: string) =>
    setOpen((o) => {
      const next = new Set(o);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  return (
    <aside class="watch">
      <input class="filter" type="search" placeholder="Filter" value={filter} onInput={(e) => setFilter(e.currentTarget.value)} />
      <div class="rows" role="tree">
        {rows.map(({ node, depth }) => (
          <Row
            key={node.id}
            node={node}
            depth={depth}
            open={!!q || open.has(node.id)}
            connected={connected}
            slot={slots.get(node.id)}
            full={traces.length >= SLOTS}
            onFlip={() => flip(node.id)}
            onToggle={() => onToggle(node.id)}
          />
        ))}
      </div>
    </aside>
  );
}

function Row(props: {
  node: Node;
  depth: number;
  open: boolean;
  connected: boolean;
  slot: number | undefined;
  full: boolean;
  onFlip: () => void;
  onToggle: () => void;
}) {
  const { node, depth, open, connected, slot, full } = props;
  const [editing, setEditing] = useState(false);
  const value = node.read?.();
  const plottable = typeof value === "number";
  const plotted = slot !== undefined;

  return (
    <div
      class="row"
      role="treeitem"
      aria-expanded={node.children ? open : undefined}
      style={{ paddingLeft: `${depth * 14 + 6}px` }}
      onClick={node.children ? props.onFlip : undefined}
      title={node.type}
    >
      <span class="chevron">{node.children ? (open ? "▾" : "▸") : ""}</span>
      <span class="name">{node.name}</span>
      {node.children && <span class="type">{node.type}</span>}
      <span class="grow" />
      {value !== undefined &&
        (editing ? (
          <input
            class="edit"
            autoFocus
            defaultValue={format(value)}
            onClick={(e) => e.stopPropagation()}
            onBlur={() => setEditing(false)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setEditing(false);
              if (e.key !== "Enter") return;
              const v = Number(e.currentTarget.value);
              if (Number.isFinite(v)) node.write!(v);
              setEditing(false);
            }}
          />
        ) : (
          <span
            class={`value ${node.write && connected ? "writable" : ""} ${typeof value === "string" ? "text" : ""}`}
            onDblClick={node.write && connected ? () => setEditing(true) : undefined}
            title={node.write ? "Double-click to change" : undefined}
          >
            {connected ? format(value) : "—"}
            {node.unit && <span class="unit">{node.unit}</span>}
          </span>
        ))}
      <span class="plot-slot">
        {plottable && (
          <button
            class={`swatch ${plotted ? "on" : ""}`}
            style={plotted ? { "--c": `var(--series-${slot + 1})` } : undefined}
            disabled={!plotted && full}
            aria-pressed={plotted}
            aria-label={plotted ? `Stop plotting ${node.id}` : `Plot ${node.id}`}
            onClick={(e) => {
              e.stopPropagation();
              props.onToggle();
            }}
          />
        )}
      </span>
    </div>
  );
}

// Plot: one lane per unit, cursors synchronised

function label(id: string) {
  return id.replace(/^control\./, "");
}

function Plot({ traces, paused }: { traces: Trace[]; paused: boolean }) {
  const groups = useMemo(() => {
    const by = new Map<string, Trace[]>();
    for (const t of traces) {
      const unit = byId.get(t.id)!.unit ?? "";
      by.set(unit, [...(by.get(unit) ?? []), t]);
    }
    return [...by.entries()];
  }, [traces]);

  if (groups.length === 0) return <section class="plot empty">Plot a value with its square in Live Watch.</section>;
  return (
    <section class={`plot ${paused ? "paused" : ""}`}>
      {groups.map(([unit, ts], i) => (
        <Lane key={unit + ts.map((t) => t.id).join()} unit={unit} traces={ts} last={i === groups.length - 1} />
      ))}
    </section>
  );
}

function css(name: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function Lane({ unit, traces, last }: { unit: string; traces: Trace[]; last: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current!;
    const ids = traces.map((t) => t.id);
    const grid = css("--grid");
    const muted = css("--muted");
    const axis = { stroke: muted, grid: { stroke: grid, width: 1 }, ticks: { stroke: grid, width: 1 }, font: "11px var(--mono)" };
    const plot = new uPlot(
      {
        width: el.clientWidth,
        height: el.clientHeight,
        cursor: { sync: { key: "lanes" }, drag: { x: true, y: false }, points: { size: 6 } },
        // Above the lane, so every lane's names line up with its own traces
        legend: { live: true, mount: (_u, legend) => el.prepend(legend) },
        scales: { x: { time: false, range: (_u, min, max) => (view.paused ? [min, max] : [view.from, view.to]) } },
        axes: [
          { ...axis, show: true, size: last ? 28 : 8, values: (_u, splits) => (last ? splits.map((v) => `${(v - view.to).toFixed(1)} s`) : splits.map(() => "")) },
          { ...axis, size: 64, label: unit, labelSize: 16, labelFont: "11px var(--mono)" },
        ],
        series: [
          { label: "t", value: (_u, v) => (v == null ? "" : `${(v - view.to).toFixed(3)} s`) },
          ...traces.map((t) => ({
            label: label(t.id),
            stroke: css(`--series-${t.slot + 1}`),
            width: 2,
            points: { show: false },
            value: (_u: uPlot, v: number | null) => (v == null ? "—" : format(v)),
          })),
        ],
      },
      history.window(ids, view.from, view.to),
      el,
    );
    const legend = el.querySelector<HTMLElement>(".u-legend");
    const fit = () => plot.setSize({ width: el.clientWidth, height: Math.max(60, el.clientHeight - (legend?.offsetHeight ?? 0)) });
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    const update = () => {
      plot.setData(history.window(ids, view.from, view.to));
      // Without a cursor, the legend reads the newest sample
      if (plot.cursor.idx == null) plot.setLegend({ idx: plot.data[0].length - 1 });
    };
    lanes.add(update);
    return () => {
      lanes.delete(update);
      observer.disconnect();
      plot.destroy();
    };
  }, [traces, last, unit]);
  return <div class="lane" ref={box} />;
}

render(<App />, document.getElementById("root")!);
