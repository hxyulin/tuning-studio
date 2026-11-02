import { isObject } from './launchConfig';

export type ValueKind = 'param' | 'watch_mut' | 'watch' | 'raw';

export interface InspectValue {
  path: string;
  type: string;
  kind: ValueKind;
  writable: boolean;
  display: string;
  number?: number;
  error?: string;
  writeReason?: string;
}

export interface InspectResult {
  version: 1;
  values: InspectValue[];
}

const KINDS: readonly string[] = ['param', 'watch_mut', 'watch', 'raw'];

/** Parse and check `tuning-studio inspect` output. Throws on any mismatch. */
export function parseInspect(stdout: string): InspectResult {
  let json: unknown;
  try {
    json = JSON.parse(stdout);
  } catch {
    throw new Error('inspect did not print JSON');
  }
  if (!isObject(json)) {
    throw new Error('inspect output is not a JSON object');
  }
  if (json.version !== 1) {
    throw new Error(`inspect output version ${String(json.version)} is not supported (expected 1)`);
  }
  if (!Array.isArray(json.values)) {
    throw new Error('inspect output has no values array');
  }
  const values = json.values.map((entry: unknown, i: number): InspectValue => {
    if (!isObject(entry)) {
      throw new Error(`inspect value ${i} is not an object`);
    }
    const { path, type, kind, writable, display, number, error, write_reason } = entry;
    if (typeof path !== 'string' || path === '') {
      throw new Error(`inspect value ${i} has no path`);
    }
    if (typeof type !== 'string') {
      throw new Error(`inspect value ${path} has no type`);
    }
    if (typeof kind !== 'string' || !KINDS.includes(kind)) {
      throw new Error(`inspect value ${path} has unknown kind ${String(kind)}`);
    }
    if (typeof writable !== 'boolean' || typeof display !== 'string') {
      throw new Error(`inspect value ${path} is missing writable or display`);
    }
    if (number !== undefined && number !== null && typeof number !== 'number') {
      throw new Error(`inspect value ${path} has a non-numeric number`);
    }
    if (error !== undefined && error !== null && typeof error !== 'string') {
      throw new Error(`inspect value ${path} has a non-string error`);
    }
    const value: InspectValue = { path, type, kind: kind as ValueKind, writable, display };
    if (typeof number === 'number' && Number.isFinite(number)) {
      value.number = number;
    }
    if (typeof error === 'string') {
      value.error = error;
    }
    if (typeof write_reason === 'string') value.writeReason = write_reason;
    return value;
  });
  return { version: 1, values };
}

/**
 * Assign the values of one `inspect <locations...>` call to the locations asked
 * for. A value belongs to a location when its path is the location or a field
 * or element under it. When no path matches but the counts agree, values are
 * taken in order, in case the CLI normalized the paths.
 */
export function matchLocations(
  locations: readonly string[],
  values: readonly InspectValue[],
): Map<string, InspectValue[]> {
  const matched = new Map<string, InspectValue[]>();
  let any = false;
  for (const location of locations) {
    const own = values.filter(
      (v) =>
        v.path === location ||
        v.path.startsWith(location + '.') ||
        v.path.startsWith(location + '['),
    );
    any ||= own.length > 0;
    matched.set(location, own);
  }
  if (!any && values.length === locations.length) {
    locations.forEach((location, i) => matched.set(location, [values[i]]));
  }
  return matched;
}

/** One line for a tree row; the full text goes in the tooltip. */
export function singleLine(display: string, max = 120): string {
  const line = display.replace(/\s+/g, ' ').trim();
  return line.length > max ? line.slice(0, max - 1) + '…' : line;
}

/** Explain edit availability even when used with an older CLI. */
export function editReason(value: InspectValue): string {
  if (value.error) return value.error;
  if (value.writeReason) return value.writeReason;
  if (!value.writable) return value.kind === 'watch'
    ? 'watch! publishes a read-only snapshot. Use watch_mut! with a mutable scalar to accept edits.'
    : 'This location cannot be written. Select a scalar field or expose it with watch_mut!.';
  if (value.kind === 'watch_mut') return 'Applied when the firmware next calls watch_mut!.';
  if (value.kind === 'param') return 'The firmware applies its range and step limits.';
  return 'Direct memory write. Task-local edits can race a task state change or be overwritten by the firmware.';
}
