/**
 * The two things the extension needs from `.cargo/config.toml`: the build target
 * and each target's runner. This reads only those keys, not general TOML.
 */
export interface CargoConfig {
  /** `build.target`, which may list several triples. */
  buildTargets: string[];
  /** `target.<triple>.runner`, split into words as Cargo splits a string runner. */
  runners: Map<string, string[]>;
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
      config.buildTargets = typeof value === 'string' ? [value] : value;
    } else if (key.length === 3 && key[0] === 'target' && key[2] === 'runner') {
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
