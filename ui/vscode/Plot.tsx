import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { formatTick, formatTicks } from "../../src/live/format";
import { samples } from "../../src/live/samples";
import { type Watch, watchName } from "../../src/live/useWatches";

/** The time range every lane shows; `to` follows the newest sample unless paused */
const view = { from: 0, to: 0, span: 5, paused: false };
const lanes = new Set<() => void>();

function css(name: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Counts theme changes: the host sets `data-theme`, and VS Code restyles the root */
function useTheme() {
  const [theme, setTheme] = useState(0);
  useEffect(() => {
    const observer = new MutationObserver(() => setTheme((n) => n + 1));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "style"] });
    return () => observer.disconnect();
  }, []);
  return theme;
}

export function Plot({ watches, span, paused }: { watches: Watch[]; span: number; paused: boolean }) {
  view.span = span;
  view.paused = paused;
  const theme = useTheme();

  // One lane per unit; values without a unit share one
  const groups = useMemo(() => {
    const by = new Map<string, Watch[]>();
    for (const w of watches) {
      if (!w.plotted || w.trace === null) continue;
      const unit = w.unit ?? "";
      by.set(unit, [...(by.get(unit) ?? []), w]);
    }
    return [...by];
  }, [watches]);

  useEffect(() => {
    let raf = 0;
    let drawn = -1;
    const draw = () => {
      const fresh = samples.version !== drawn;
      if (!view.paused && fresh) {
        drawn = samples.version;
        view.to = Math.max(samples.latestTime, view.span);
        view.from = view.to - view.span;
        lanes.forEach((update) => update());
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  // A new window or resume redraws at once
  useEffect(() => {
    if (paused) return;
    view.to = Math.max(samples.latestTime, span);
    view.from = view.to - span;
    lanes.forEach((update) => update());
  }, [span, paused]);

  if (groups.length === 0) return <section class="plot empty">Plot a value with its square in Live Watch.</section>;
  return (
    <section class={`plot ${paused ? "paused" : ""}`}>
      {groups.map(([unit, ws], i) => (
        <Lane key={`${unit}|${ws.map((w) => `${w.id}:${w.trace}`).join()}`} unit={unit} watches={ws} last={i === groups.length - 1} theme={theme} />
      ))}
    </section>
  );
}

/** Colours are read when the lane is built, so a theme change rebuilds it */
function Lane({ unit, watches, last, theme }: { unit: string; watches: Watch[]; last: boolean; theme: number }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = box.current!;
    const ids = watches.map((w) => w.id);
    const grid = css("--grid");
    const axis = { stroke: css("--muted"), grid: { stroke: grid, width: 1 }, ticks: { stroke: grid, width: 1 }, font: `11px ${css("--mono")}` };
    const shaped = () => samples.window(ids, view.from, view.to, Math.max(50, Math.floor(el.clientWidth))).data;
    const plot = new uPlot(
      {
        width: el.clientWidth,
        height: el.clientHeight,
        cursor: { sync: { key: "lanes" }, drag: { x: true, y: false }, points: { size: 6 } },
        // Above the lane, so each lane's names sit with its own traces
        legend: { live: true, mount: (_u, legend) => el.prepend(legend) },
        scales: { x: { time: false, range: (_u, min, max) => (view.paused ? [min, max] : [view.from, view.to]) } },
        axes: [
          { ...axis, size: last ? 28 : 8, values: (_u, splits) => splits.map((v) => (last ? `${(v - view.to).toFixed(1)} s` : "")) },
          // Large values as exponents, which fit the axis; uPlot's default groups digits and overflows
          { ...axis, size: 64, label: unit || undefined, labelSize: unit ? 16 : 0, labelFont: `11px ${css("--mono")}`, values: (_u, splits, _i, _space, step) => formatTicks(splits, step) },
        ],
        series: [
          { label: "t", value: (_u, v) => (v == null ? "" : `${(v - view.to).toFixed(3)} s`) },
          ...watches.map((w) => ({
            label: watchName(w.path),
            stroke: css(`--series-${(w.trace ?? 0) + 1}`),
            width: 2,
            points: { show: false },
            value: (_u: uPlot, v: number | null) => (v == null ? "—" : formatTick(v)),
          })),
        ],
      },
      shaped(),
      el,
    );
    const legend = el.querySelector<HTMLElement>(".u-legend");
    const fit = () => plot.setSize({ width: el.clientWidth, height: Math.max(60, el.clientHeight - (legend?.offsetHeight ?? 0)) });
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    const update = () => {
      plot.setData(shaped());
      // Without a cursor, the legend reads the newest sample
      if (plot.cursor.idx == null) plot.setLegend({ idx: plot.data[0].length - 1 });
    };
    lanes.add(update);
    return () => {
      lanes.delete(update);
      observer.disconnect();
      plot.destroy();
    };
    // Watches are keyed by id and trace slot in the parent
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [unit, last, theme]);
  return <div class="lane" ref={box} />;
}
