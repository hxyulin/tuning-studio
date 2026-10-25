import { useCallback, useEffect, useState } from "react";
import type { CatalogEntry, NodeRef, OpenedElf, Scalar, SymbolNode } from "../elf/api";
import { host } from "../host";
import { samples } from "./samples";

export const MAX_TRACES = 8;

export interface Watch {
  id: number;
  /** The symbol sampled; null for a tuning table value */
  ref: NodeRef | null;
  /** Tuning table value id, sampled as the value the firmware applies */
  cell: number | null;
  path: string;
  typeName: string;
  scalar: Scalar | null;
  plotted: boolean;
  /** Groups traces into lanes; a tuning value's comes from the firmware, a symbol's from the user */
  unit: string | null;
  /** Set when the ELF cannot sample this value */
  error: string | null;
  /** Trace colour slot, stable while plotted */
  trace: number | null;
}

type Stored = Pick<Watch, "ref" | "cell" | "path" | "typeName" | "scalar" | "plotted" | "unit">;

/** A value to add, with its lane unit and whether to plot it when there is room */
export type NewWatch = Omit<Watch, "id" | "plotted" | "error" | "trace"> & { plotted?: boolean };

const storageKey = (elfPath: string) => `watches:${elfPath}`;

function load(elfPath: string): Stored[] {
  try {
    const stored: Stored[] = JSON.parse(host.storage.get(storageKey(elfPath)) ?? "[]");
    return stored.map((s) => ({ ...s, ref: s.ref ?? null, cell: s.cell ?? null, unit: s.unit ?? null }));
  } catch {
    return [];
  }
}

let nextId = 1;

/** A watch's short name, as the legend shows it: the path after its module */
export function watchName(path: string): string {
  // A task local, `app::control_task::POOL[task 0].future#3.pid`, reads as `control_task.pid`
  const local = path.replace(/::POOL\[task (\d+)\]\.future#\d+/, (_, slot) => (slot === "0" ? "" : `[${slot}]`));
  const cut = local.lastIndexOf("::");
  return cut < 0 ? local : local.slice(cut + 2);
}

function assignTraces(watches: Watch[]): Watch[] {
  const used = new Set(watches.filter((w) => w.plotted && w.trace !== null).map((w) => w.trace));
  return watches.map((w) => {
    if (!w.plotted) return w.trace === null ? w : { ...w, trace: null };
    if (w.trace !== null) return w;
    let slot = 0;
    while (used.has(slot)) slot++;
    used.add(slot);
    return { ...w, trace: slot };
  });
}

/**
 * The watched set, kept in sync with the session. `owner` names the list: the
 * ELF path, or a fixed key for a link with no ELF.
 */
export function useWatches(owner: string | null, elf: OpenedElf | null) {
  const elfPath = owner;
  // The list remembers which ELF it belongs to, so a switch never saves one ELF's list under another
  const [state, setState] = useState<{ owner: string | null; list: Watch[] }>({ owner: null, list: [] });
  const watches = state.owner === elfPath ? state.list : [];
  const setWatches = useCallback(
    (update: (ws: Watch[]) => Watch[]) => setState((s) => ({ ...s, list: update(s.list) })),
    [],
  );

  // Restore the list saved for this ELF
  useEffect(() => {
    if (!elfPath) return;
    setState({
      owner: elfPath,
      list: assignTraces(load(elfPath).map((s) => ({ ...s, id: nextId++, error: null, trace: null }))),
    });
  }, [elfPath]);

  // Push the set to the backend whenever membership or a unit changes (recordings and the stream
  // carry units); plot toggles do not matter to it
  const ready = elfPath !== null && state.owner === elfPath;
  const membership = JSON.stringify(watches.map((w) => [w.id, w.unit]));
  useEffect(() => {
    if (!ready) return;
    host
      .setWatches(
        watches.map((w) => ({
          id: w.id,
          node: w.ref,
          cell: w.cell,
          name: watchName(w.path),
          unit: w.unit,
          path: w.path,
          typeName: w.typeName,
        })),
      )
      .then((results) => {
        const errors = new Map(results.map((r) => [r.id, r.error]));
        setWatches((ws) => ws.map((w) => (errors.has(w.id) ? { ...w, error: errors.get(w.id) ?? null } : w)));
      })
      .catch(() => {});
    // Re-resolve on every ELF load too: a rebuild moves addresses
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [membership, elf, ready]);

  // Save
  useEffect(() => {
    if (!ready || !elfPath) return;
    const stored: Stored[] = watches.map(({ ref, cell, path, typeName, scalar, plotted, unit }) => ({
      ref,
      cell,
      path,
      typeName,
      scalar,
      plotted,
      unit,
    }));
    try {
      host.storage.set(storageKey(elfPath), JSON.stringify(stored));
    } catch {
      // Storage full or unavailable: the list still works for this run
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state, ready]);

  const addWatches = useCallback((items: NewWatch[], onlyIfEmpty = false) => {
    setWatches((ws) => {
      if (onlyIfEmpty && ws.length) return ws;
      const have = new Set(ws.map((w) => w.path));
      let plotted = ws.filter((w) => w.plotted).length;
      const added = items
        .filter((item) => !have.has(item.path))
        .map((item) => {
          const plot = item.plotted !== false && plotted < MAX_TRACES;
          if (plot) plotted++;
          return { ...item, id: nextId++, plotted: plot, error: null, trace: null };
        });
      return assignTraces(ws.concat(added));
    });
  }, []);

  const add = useCallback(
    (nodes: SymbolNode[]) =>
      addWatches(
        nodes.map((n) => ({ ref: n.ref, cell: null, path: n.path, typeName: n.typeName, scalar: n.scalar, unit: null })),
      ),
    [addWatches],
  );

  /** Start a list that has nothing saved, e.g. from the host's launch configuration */
  const seed = useCallback((items: NewWatch[]) => addWatches(items, true), [addWatches]);

  const addCell = useCallback(
    (entry: CatalogEntry) =>
      addWatches([
        {
          ref: null,
          cell: entry.id,
          path: entry.name,
          typeName: entry.unit ? `${entry.kind}, ${entry.unit}` : entry.kind,
          scalar: entry.kind,
          unit: entry.unit || null,
        },
      ]),
    [addWatches],
  );

  const remove = useCallback((id: number) => {
    samples.forget(id);
    setWatches((ws) => ws.filter((w) => w.id !== id));
  }, []);

  const clear = useCallback(() => {
    setWatches((ws) => {
      ws.forEach((w) => samples.forget(w.id));
      return [];
    });
  }, []);

  const togglePlot = useCallback((id: number) => {
    setWatches((ws) => {
      const plotted = ws.filter((w) => w.plotted).length;
      return assignTraces(
        ws.map((w) =>
          w.id !== id ? w : w.plotted ? { ...w, plotted: false } : plotted < MAX_TRACES ? { ...w, plotted: true } : w,
        ),
      );
    });
  }, []);

  const setUnit = useCallback((id: number, unit: string | null) => {
    setWatches((ws) => ws.map((w) => (w.id === id ? { ...w, unit } : w)));
  }, []);

  return { watches, ready, add, addCell, seed, remove, clear, togglePlot, setUnit };
}
