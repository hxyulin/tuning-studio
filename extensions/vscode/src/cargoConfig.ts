import * as path from 'node:path';

/**
 * The two things the extension needs from `.cargo/config.toml`: the build target
 * and each target's runner. This reads only those keys, not general TOML.
 */
export interface CargoConfig {
  /** `build.target`, which may list several triples. */
  buildTargets: string[];
  /** `target.<triple>.runner`, split into words as Cargo splits a string runner. */
  runners: Map<string, string[]>;
  buildTargetKind?: 'string' | 'array';
  runnerKinds?: Map<string, 'string' | 'array'>;
  buildSource?: string;
  runnerSources?: Map<string, string>;
}

export function parseCargoConfig(text: string): CargoConfig {
  const config: CargoConfig = { buildTargets: [], runners: new Map() };
  let table: string[] = [];
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    let line = stripComment(lines[i]).trim();
    if (line === '') {
      continue;
    }
    const header = /^\[\s*([^\[\]]+?)\s*\]$/.exec(line);
    if (header) {
      table = splitKey(header[1]);
      continue;
    }
    if (line.startsWith('[[')) {
      table = ['<array-table>'];
      continue;
    }
    const eq = line.indexOf('=');
    if (eq < 0) {
      continue;
    }
    // A multi-line array continues until its brackets balance.
    while (opens(line) > 0 && i + 1 < lines.length) {
      i++;
      line += ' ' + stripComment(lines[i]).trim();
    }
    const key = [...table, ...splitKey(line.slice(0, eq))];
    const value = parseValue(line.slice(eq + 1).trim());
    if (value === undefined) {
      continue;
    }
    if (key.length === 2 && key[0] === 'build' && key[1] === 'target') {
      config.buildTargetKind = typeof value === 'string' ? 'string' : 'array';
      config.buildTargets = typeof value === 'string' ? [value] : value;
    } else if (key.length === 3 && key[0] === 'target' && key[2] === 'runner') {
      (config.runnerKinds ??= new Map()).set(key[1], typeof value === 'string' ? 'string' : 'array');
      config.runners.set(key[1], typeof value === 'string' ? splitWords(value) : value);
    }
  }
  return config;
}

/** The environment variable Cargo reads a target's runner from. */
export function runnerEnvName(triple: string): string {
  return `CARGO_TARGET_${triple.toUpperCase().replace(/[-.]/g, '_')}_RUNNER`;
}

/** The value of `--chip` in a runner's words, if there is one. */
export function chipFromRunner(words: readonly string[]): string | undefined {
  for (let i = 0; i < words.length; i++) {
    if (words[i] === '--chip' && i + 1 < words.length) {
      return words[i + 1];
    }
    if (words[i].startsWith('--chip=')) {
      return words[i].slice('--chip='.length);
    }
  }
  return undefined;
}

/** Cargo splits a string runner on whitespace, with no quoting. */
export function splitWords(value: string): string[] {
  return value.split(/\s+/).filter((w) => w !== '');
}

function stripComment(line: string): string {
  let quote: string | undefined;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      if (c === '\\' && quote === '"') {
        i++;
      } else if (c === quote) {
        quote = undefined;
      }
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === '#') {
      return line.slice(0, i);
    }
  }
  return line;
}

function opens(line: string): number {
  let depth = 0;
  let quote: string | undefined;
  for (let i = line.indexOf('='); i < line.length; i++) {
    const c = line[i];
    if (quote) {
      if (c === '\\' && quote === '"') {
        i++;
      } else if (c === quote) {
        quote = undefined;
      }
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === '[') {
      depth++;
    } else if (c === ']') {
      depth--;
    }
  }
  return depth;
}

/** Split a dotted key, keeping quoted parts such as `'cfg(...)'` whole. */
function splitKey(key: string): string[] {
  const parts: string[] = [];
  const re = /\s*(?:"((?:[^"\\]|\\.)*)"|'([^']*)'|([A-Za-z0-9_-]+))\s*(?:\.|$)/y;
  let match: RegExpExecArray | null;
  while (re.lastIndex < key.length && (match = re.exec(key)) !== null) {
    parts.push(match[1] ?? match[2] ?? match[3]);
  }
  return parts;
}

function parseValue(text: string): string | string[] | undefined {
  const single = parseString(text);
  if (single !== undefined) {
    return single.value;
  }
  if (!text.startsWith('[')) {
    return undefined;
  }
  const items: string[] = [];
  let rest = text.slice(1).trim();
  while (!rest.startsWith(']')) {
    const item = parseString(rest);
    if (item === undefined) {
      return undefined;
    }
    items.push(item.value);
    rest = item.rest.trim();
    if (rest.startsWith(',')) {
      rest = rest.slice(1).trim();
    } else if (!rest.startsWith(']')) {
      return undefined;
    }
  }
  return items;
}

function parseString(text: string): { value: string; rest: string } | undefined {
  if (text.startsWith("'")) {
    const end = text.indexOf("'", 1);
    return end < 0 ? undefined : { value: text.slice(1, end), rest: text.slice(end + 1) };
  }
  if (text.startsWith('"')) {
    let value = '';
    for (let i = 1; i < text.length; i++) {
      const c = text[i];
      if (c === '"') {
        return { value, rest: text.slice(i + 1) };
      }
      if (c === '\\') {
        i++;
        const escaped: Record<string, string> = { n: '\n', t: '\t', '"': '"', '\\': '\\' };
        value += escaped[text[i]] ?? text[i];
      } else {
        value += c;
      }
    }
  }
  return undefined;
}

/** Read the supported Cargo keys from Cargo home, then ancestors, then cwd. */
export async function discoverCargoConfig(
  cwd: string,
  cargoHome: string,
  read: (file: string) => Promise<string | undefined>,
): Promise<CargoConfig> {
  const merged: CargoConfig = { buildTargets: [], runners: new Map(), runnerKinds: new Map(), runnerSources: new Map() };
  for (const dir of cargoConfigDirectories(cwd, cargoHome)) {
    // Cargo prefers the legacy filename when both exist.
    let file = path.join(dir, 'config');
    let text = await read(file);
    if (text === undefined) { file = path.join(dir, 'config.toml'); text = await read(file); }
    if (text === undefined) continue;
    const config = parseCargoConfig(text);
    if (config.buildTargetKind) {
      if (merged.buildTargetKind && merged.buildTargetKind !== config.buildTargetKind) {
        throw new Error(`Cargo build.target changes type in ${file}`);
      }
      merged.buildTargets = config.buildTargetKind === 'array'
        ? [...merged.buildTargets, ...config.buildTargets] : config.buildTargets;
      merged.buildTargetKind = config.buildTargetKind;
      merged.buildSource = file;
    }
    for (const [target, runner] of config.runners) {
      const kind = config.runnerKinds!.get(target)!;
      const previous = merged.runnerKinds!.get(target);
      if (previous && previous !== kind) throw new Error(`Cargo runner for ${target} changes type in ${file}`);
      merged.runners.set(target, kind === 'array' ? [...(merged.runners.get(target) ?? []), ...runner] : runner);
      merged.runnerKinds!.set(target, kind);
      merged.runnerSources!.set(target, file);
    }
  }
  return merged;
}

/** Configuration directories from lowest to highest precedence, without duplicates. */
export function cargoConfigDirectories(cwd: string, cargoHome: string): string[] {
  const dirs: string[] = [];
  for (let dir = path.resolve(cwd);; dir = path.dirname(dir)) {
    dirs.push(path.join(dir, '.cargo'));
    if (path.dirname(dir) === dir) break;
  }
  const ordered = [path.resolve(cargoHome), ...dirs.reverse()];
  return ordered.filter((dir, i) => ordered.lastIndexOf(dir) === i);
}
