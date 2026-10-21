import type { Scalar } from "../elf/api";

export function formatValue(value: number | undefined, scalar: Scalar | null): string {
  if (value === undefined) return "";
  if (Number.isNaN(value)) return "read failed";
  if (scalar === "bool") return value ? "true" : "false";
  if (scalar === "f32" || scalar === "f64") {
    if (value === 0) return "0";
    const abs = Math.abs(value);
    return abs >= 1e6 || abs < 1e-4 ? value.toExponential(4) : String(Number(value.toPrecision(6)));
  }
  return String(value);
}

export function formatRate(hz: number): string {
  return hz >= 100 ? hz.toFixed(0) : hz.toFixed(1);
}

export function formatMicros(us: number): string {
  return us >= 1000 ? `${(us / 1000).toFixed(1)} ms` : `${Math.round(us)} µs`;
}

/**
 * A reading that changes many times a second: floats get a fixed number of decimals for their
 * size, so the digits hold still while the value moves.
 */
export function formatLive(value: number | null | undefined, scalar: Scalar | null): string {
  if (value === null || value === undefined || Number.isNaN(value)) return "–";
  if (scalar !== "f32" && scalar !== "f64") return formatValue(value, scalar);
  return formatTick(value);
}

/** Axis ticks `step` apart, all with the decimals the step needs */
export function formatTicks(ticks: number[], step: number): string[] {
  // As many decimals as the step itself has: 0.25 needs two, 0.5 one
  let decimals = 0;
  while (decimals < 6 && Math.abs(Math.round(step * 10 ** decimals) - step * 10 ** decimals) > 1e-6) decimals++;
  const big = ticks.some((t) => Math.abs(t) >= 1e6);
  return ticks.map((t) => (big ? t.toExponential(1) : t.toFixed(decimals)));
}

/** A float reading: fewer decimals as the magnitude grows */
export function formatTick(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 1e6 || (abs > 0 && abs < 1e-3)) return value.toExponential(2);
  return value.toFixed(abs >= 1000 ? 0 : abs >= 100 ? 1 : abs >= 1 ? 2 : 3);
}
