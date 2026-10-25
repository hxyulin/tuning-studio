// Fixed-size sample history for every numeric leaf, sampled at 500 Hz.
import { numeric } from "./tree";

export const SAMPLE_HZ = 500;
const CAPACITY = SAMPLE_HZ * 30;

const time = new Float64Array(CAPACITY);
const values = new Map(numeric.map((n) => [n.id, new Float64Array(CAPACITY)]));
let written = 0;

export function record(t: number) {
  const at = written % CAPACITY;
  time[at] = t;
  for (const n of numeric) values.get(n.id)![at] = n.read!() as number;
  written++;
}

export function clear() {
  written = 0;
}

/** Samples from `from` to `to` seconds, as uPlot's aligned columns */
export function window(ids: string[], from: number, to: number): [number[], ...number[][]] {
  const count = Math.min(written, CAPACITY);
  const first = written - count;
  const xs: number[] = [];
  const ys: number[][] = ids.map(() => []);
  const columns = ids.map((id) => values.get(id)!);
  for (let i = first; i < written; i++) {
    const at = i % CAPACITY;
    const t = time[at];
    if (t < from || t > to) continue;
    xs.push(t);
    columns.forEach((c, k) => ys[k].push(c[at]));
  }
  return [xs, ...ys];
}
