import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { type Catalog, type CatalogEntry, type NodeRef, type RootNode, type SymbolNode, type Task, type TaskPoint, shortLocation } from "../../src/elf/api";
import type { TaskStatus, ValueRead } from "../../src/live/api";
import { formatValue } from "../../src/live/format";
import { tuningEntry, tuningNode } from "../../src/live/tuningPresentation";
import type { Tune } from "../../src/live/useSession";
import type { Watch } from "../../src/live/useWatches";
import { MAX_TRACES } from "../../src/live/useWatches";
import { host } from "./host";

const PAGE = 64;
/** Numeric fields read per poll; the rest show no value until the tree is narrowed */
const READ_LIMIT = 128;
const POLL_MS = 200;
/** Task states change at each `.await`; people read them, they do not plot them */
const TASK_POLL_MS = 500;
const TASKS = "tasks";

type Loaded = { status: "loading" } | { status: "error"; message: string } | { status: "ok"; nodes: SymbolNode[]; total: number };

/** A node, or a task state, whose children load in pages */
type Container = { path: string; ref: NodeRef };

type Row =
  | { type: "node"; node: SymbolNode; depth: number }
  | { type: "more"; of: Container; depth: number; shown: number; total: number }
  | { type: "note"; key: string; depth: number; text: string }
  | { type: "group"; key: string; name: string; count: number }
  | { type: "task"; task: Task; status: TaskStatus | undefined };

/** Statics under `<T as Trait>` or `<T>` group with T's crate */
function crateOf(segment: string) {
  if (!segment.startsWith("<")) return segment;
  return segment.replace(/^<(&|mut |\*const |\*mut )*/, "").split(/::|[<> ,]/)[0];
}

function taskKey(task: Task) {
  return `task:${task.root.path}`;
}

/** The state whose locals the task holds now: arguments before the first poll, or an `.await`'s */
function heldState(status: TaskStatus | undefined): TaskPoint | null {
  const at = status?.state?.spawned ? status.state.at : null;
  return at && at.label !== "Returned" && at.label !== "Panicked" ? at : null;
}

/** What a task is doing, in a word or a source line */
function taskText(status: TaskStatus | undefined): { text: string; tone: string; title?: string } {
  if (!status) return { text: "—", tone: "" };
  const { state } = status;
  if (!state) return { text: "read failed", tone: "failed", title: status.error ?? undefined };
  if (!state.spawned) return { text: "not running", tone: "muted" };
  const at = state.at;
  const title = state.queued ? "Woken, waiting to be polled" : undefined;
  if (!at || at.label === "Unresumed") return { text: "not started", tone: "muted", title };
  if (at.label === "Returned") return { text: "returned", tone: "muted" };
  if (at.label === "Panicked") return { text: "panicked", tone: "failed" };
  return { text: at.location ? shortLocation(at.location) : at.label, tone: "text", title: title ?? (at.location ? `${at.location.file}:${at.location.line}` : undefined) };
}

/** Plots sample fixed addresses: no pointer or slice hops */
function plottable(node: SymbolNode) {
  return node.readable && typeof node.scalar === "string" && !node.ref.steps.some((s) => s.kind === "deref" || s.kind === "sliceIndex");
}

function readable(node: SymbolNode) {
  return node.readable && (typeof node.scalar === "string" || node.kind === "enum" || node.kind === "taggedEnum" || node.kind === "pointer");
}

export function LiveWatch(props: {
  roots: RootNode[];
  tasks: Task[];
  catalog: Catalog | null;
  tune: Tune | null;
  live: boolean;
  watches: Watch[];
  onPlot: (node: SymbolNode, entry: CatalogEntry | undefined) => void;
  onRemove: (id: number) => void;
}) {
  const { roots, tasks, catalog, tune, live, watches } = props;
  const [filter, setFilter] = useState("");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [children, setChildren] = useState<Map<string, Loaded>>(() => new Map());
  const [values, setValues] = useState<Map<string, ValueRead>>(() => new Map());
  const [statuses, setStatuses] = useState<Map<string, TaskStatus>>(() => new Map());
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(!document.hidden);
  // Answers for a tree that has since been replaced are dropped
  const generation = useRef(0);

  // A new ELF, or the link coming and going, changes what children read as
  useEffect(() => {
    generation.current++;
    setChildren(new Map());
    setValues(new Map());
  }, [roots, live]);

  useEffect(() => {
    const change = () => setVisible(!document.hidden);
    document.addEventListener("visibilitychange", change);
    return () => document.removeEventListener("visibilitychange", change);
  }, []);

  const load = (node: Container, offset: number) => {
    const revision = generation.current;
    if (offset === 0) setChildren((m) => new Map(m).set(node.path, { status: "loading" }));
    host.inspectChildren(node.ref, offset, PAGE, live).then(
      (c) => {
        if (revision !== generation.current) return;
        setChildren((m) => {
          const old = m.get(node.path);
          const before = offset > 0 && old?.status === "ok" ? old.nodes : [];
          return new Map(m).set(node.path, { status: "ok", nodes: [...before, ...c.nodes], total: c.total });
        });
      },
      (e) => {
        if (revision === generation.current) setChildren((m) => new Map(m).set(node.path, { status: "error", message: String(e) }));
      },
    );
  };

  const needle = filter.trim().toLowerCase();
  const shownTasks = useMemo(
    () => tasks.filter((t) => !needle || t.root.path.toLowerCase().includes(needle)).sort((a, b) => a.name.localeCompare(b.name) || a.slot - b.slot),
    [tasks, needle],
  );
  const tasksOpen = shownTasks.length > 0 && (!!needle || open.has(TASKS));

  // Expanded nodes whose children are not loaded yet, such as after reconnecting, and the state an
  // open task is in now
  useEffect(() => {
    const byPath = new Map<string, Container>(roots.map((r) => [r.path, r]));
    for (const loaded of children.values()) if (loaded.status === "ok") loaded.nodes.forEach((n) => byPath.set(n.path, n));
    const wanted = [...open].flatMap((path) => byPath.get(path) ?? []);
    if (tasksOpen)
      for (const task of shownTasks) {
        const state = heldState(statuses.get(task.root.path));
        if (state && open.has(taskKey(task))) wanted.push(state);
      }
    for (const node of wanted) if (!children.has(node.path)) load(node, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, children, roots, statuses, tasksOpen, shownTasks]);

  useEffect(() => {
    setStatuses(new Map());
    if (!live || !visible || !tasksOpen) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const snapshot = await host.taskStates();
        if (!stopped) setStatuses(new Map(snapshot.tasks.map((t) => [t.path, t])));
      } catch (e) {
        if (!stopped) setError(`Could not read task states: ${e}`);
      } finally {
        if (!stopped) timer = setTimeout(poll, TASK_POLL_MS);
      }
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [live, visible, tasksOpen, tasks]);

  const crates = useMemo(() => {
    // Task pools show under Tasks
    const pools = new Set(tasks.map((t) => t.root.ref.symbol));
    const by = new Map<string, RootNode[]>();
    for (const r of roots) {
      if (r.internal || pools.has(r.path) || (needle && !r.path.toLowerCase().includes(needle))) continue;
      const qualified = r.segments[0].startsWith("<");
      const crate = r.segments.length > 1 ? crateOf(r.segments[0]) : "";
      // Under its crate, a static reads as its module path: `gimbal::CONTROL`
      const label = qualified ? r.path : r.segments.length > 1 ? r.segments.slice(1).join("::") : r.label;
      by.set(crate, [...(by.get(crate) ?? []), { ...r, label }]);
    }
    return [...by].sort(([a], [b]) => a.localeCompare(b));
  }, [roots, tasks, needle]);

  const rows = useMemo(() => {
    const out: Row[] = [];
    const expand = (of: Container, depth: number, parent: SymbolNode | null) => {
      const loaded = children.get(of.path);
      if (!loaded || loaded.status === "loading") out.push({ type: "note", key: `${of.path}/loading`, depth, text: "Loading…" });
      else if (loaded.status === "error") out.push({ type: "note", key: `${of.path}/error`, depth, text: loaded.message });
      else {
        walk(loaded.nodes, depth, parent);
        if (loaded.total > loaded.nodes.length) out.push({ type: "more", of, depth, shown: loaded.nodes.length, total: loaded.total });
      }
    };
    const walk = (nodes: SymbolNode[], depth: number, parent: SymbolNode | null) => {
      for (const original of nodes) {
        // A tagged enum shows only the variant it holds now
        if (parent?.kind === "taggedEnum") {
          const step = original.ref.steps[original.ref.steps.length - 1];
          if (step?.kind === "discriminant") continue;
          if (step?.kind === "variant" && step.value !== values.get(parent.path)?.activeVariant) continue;
        }
        const node = tuningNode(original, catalog);
        out.push({ type: "node", node, depth });
        if (node.expandable && open.has(node.path)) expand(node, depth + 1, node);
      }
    };
    // Each task shows the locals of the state it is in now
    if (shownTasks.length) {
      out.push({ type: "group", key: TASKS, name: "Tasks", count: shownTasks.length });
      if (tasksOpen)
        for (const task of shownTasks) {
          const status = statuses.get(task.root.path);
          out.push({ type: "task", task, status });
          if (!open.has(taskKey(task))) continue;
          const state = heldState(status);
          if (state) expand(state, 2, null);
          else out.push({ type: "note", key: `${taskKey(task)}/none`, depth: 2, text: live ? (status ? "Holds no locals now." : "Reading…") : "Connect to read its locals." });
        }
    }
    // Statics grouped by crate; filtering opens every group
    for (const [crate, members] of crates) {
      const key = `crate:${crate}`;
      out.push({ type: "group", key, name: crate || "(global)", count: members.length });
      if (needle || open.has(key)) walk(members, 1, null);
    }
    return out;
  }, [crates, shownTasks, tasksOpen, statuses, live, needle, open, children, catalog, values]);

  const reads = useMemo(
    () => rows.flatMap((r) => (r.type === "node" && readable(r.node) && !tuningEntry(r.node, catalog) ? [r.node] : [])).slice(0, READ_LIMIT),
    [rows, catalog],
  );
  const readKey = reads.map((n) => n.path).join("\n");

  useEffect(() => {
    if (!live || !visible || reads.length === 0) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const result = await host.readValues(reads.map((n) => n.ref));
        if (stopped) return;
        setValues(new Map(reads.map((n, i) => [n.path, result[i] ?? { value: null, error: "No reading returned" }])));
        setError(null);
      } catch (e) {
        if (!stopped) setError(String(e));
      } finally {
        // After each answer, so a slow probe never queues reads
        if (!stopped) timer = setTimeout(poll, POLL_MS);
      }
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readKey, live, visible]);

  const flip = (path: string) =>
    setOpen((o) => {
      const next = new Set(o);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  const plottedCount = watches.filter((w) => w.plotted).length;
  const writable = live && tune?.check.state === "matches";

  return (
    <aside class="watch">
      <input class="filter" type="search" placeholder="Filter" value={filter} onInput={(e) => setFilter(e.currentTarget.value)} />
      {error && <p class="watch-error" role="alert">{error}</p>}
      <div class="rows" role="tree" aria-label="Live Watch">
        {rows.map((row) => {
          if (row.type === "group")
            return (
              <div key={row.key} class="row" role="treeitem" aria-expanded={!!needle || open.has(row.key)} style={{ paddingLeft: "6px" }} onClick={() => flip(row.key)}>
                <span class="chevron">{needle || open.has(row.key) ? "▾" : "▸"}</span>
                <span class="name">{row.name}</span>
                <span class="type">{row.count}</span>
              </div>
            );
          if (row.type === "task") {
            const key = taskKey(row.task);
            const state = live ? taskText(row.status) : { text: "—", tone: "" };
            const name = row.task.slots > 1 ? `${row.task.name}[${row.task.slot}]` : row.task.name;
            return (
              <div key={key} class="row" role="treeitem" aria-expanded={open.has(key)} style={{ paddingLeft: "20px" }} onClick={() => flip(key)} title={row.task.root.path}>
                <span class="chevron">{open.has(key) ? "▾" : "▸"}</span>
                <span class="name">{name}</span>
                <span class="grow" />
                <span class={`value ${state.tone}`} title={state.title}>
                  {state.text}
                </span>
                <span class="plot-slot" />
              </div>
            );
          }
          if (row.type === "note")
            return (
              <div key={row.key} class="row note" style={{ paddingLeft: `${row.depth * 14 + 22}px` }}>
                {row.text}
              </div>
            );
          if (row.type === "more")
            return (
              <div key={`${row.of.path}/more`} class="row note" style={{ paddingLeft: `${row.depth * 14 + 22}px` }}>
                <button class="link" onClick={() => load(row.of, row.shown)}>
                  Show {Math.min(PAGE, row.total - row.shown)} more of {row.total}
                </button>
              </div>
            );
          const { node } = row;
          const entry = tuningEntry(node, catalog);
          const watch = watches.find((w) => w.path === (entry ? entry.name : node.path));
          return (
            <Row
              key={node.path}
              node={node}
              depth={row.depth}
              open={open.has(node.path)}
              live={live}
              read={entry ? undefined : values.get(node.path)}
              entry={entry}
              applied={entry ? tune?.values.get(entry.id)?.applied ?? null : null}
              writable={!!writable && !!entry && entry.access !== "readOnly"}
              watch={watch}
              full={plottedCount >= MAX_TRACES}
              onFlip={() => flip(node.path)}
              onToggle={() => (watch ? props.onRemove(watch.id) : props.onPlot(node, entry))}
            />
          );
        })}
      </div>
    </aside>
  );
}

function Row(props: {
  node: SymbolNode;
  depth: number;
  open: boolean;
  live: boolean;
  read: ValueRead | undefined;
  entry: CatalogEntry | undefined;
  applied: number | null;
  writable: boolean;
  watch: Watch | undefined;
  full: boolean;
  onFlip: () => void;
  onToggle: () => void;
}) {
  const { node, depth, open, live, read, entry, watch, writable } = props;
  const [editing, setEditing] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const slot = watch?.plotted ? watch.trace : null;

  let text: string | null = null;
  let tone = "";
  if (entry) text = props.applied === null ? "—" : formatValue(props.applied, entry.kind);
  else if (readable(node)) {
    if (!live || !read) text = "—";
    else if (read.error) {
      text = "read failed";
      tone = "failed";
    } else if (read.text != null) {
      text = read.text;
      tone = "text";
    } else if (read.activeVariant != null) {
      text = read.activeVariant;
      tone = "text";
    } else if (read.value === null) text = "—";
    else text = node.kind === "pointer" ? `0x${read.value.toString(16)}` : formatValue(read.value, node.scalar);
  }

  const send = (raw: string) => {
    const value = Number(raw);
    setEditing(false);
    if (!entry || !Number.isFinite(value)) return;
    host.requestValue(entry.id, value).then(() => setProblem(null), (e) => setProblem(String(e)));
  };

  return (
    <div
      class="row"
      role="treeitem"
      aria-expanded={node.expandable ? open : undefined}
      style={{ paddingLeft: `${depth * 14 + 6}px` }}
      onClick={node.expandable ? props.onFlip : undefined}
      title={read?.error ?? problem ?? `${node.typeName} · ${node.path}`}
    >
      <span class="chevron">{node.expandable ? (open ? "▾" : "▸") : ""}</span>
      <span class="name">{node.label}</span>
      {node.expandable && <span class="type">{node.typeName}</span>}
      <span class="grow" />
      {text !== null &&
        (editing ? (
          <input
            class="edit"
            autoFocus
            defaultValue={text}
            onClick={(e) => e.stopPropagation()}
            onBlur={() => setEditing(false)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setEditing(false);
              if (e.key === "Enter") send(e.currentTarget.value);
            }}
          />
        ) : (
          <span
            class={`value ${tone} ${writable ? "writable" : ""} ${problem ? "failed" : ""}`}
            onDblClick={writable ? () => setEditing(true) : undefined}
          >
            {text}
            {entry?.unit && <span class="unit">{entry.unit}</span>}
          </span>
        ))}
      <span class="plot-slot">
        {(plottable(node) || entry) && (
          <button
            class={`swatch ${slot !== null ? "on" : ""}`}
            style={slot !== null ? { "--c": `var(--series-${slot + 1})` } : undefined}
            disabled={!watch && props.full}
            aria-pressed={!!watch}
            aria-label={watch ? `Stop plotting ${node.path}` : `Plot ${node.path}`}
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
