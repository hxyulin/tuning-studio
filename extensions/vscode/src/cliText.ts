/** Argument builders and output parsers for the tuning-studio CLI. */

/** `--probe` when a selector is configured. Never `--chip` or `--elf`, so a
 * command reaches a running session and cannot attach on its own. */
function probeArgs(probe: string): string[] {
  return probe.trim() ? ['--probe', probe.trim()] : [];
}

export function inspectArgs(probe: string, locations: readonly string[] = []): string[] {
  return locations.length > 0
    ? ['inspect', ...probeArgs(probe), '--', ...locations]
    : ['inspect', ...probeArgs(probe)];
}

export function setArgs(probe: string, location: string, value: string): string[] {
  return ['set', ...probeArgs(probe), '--', location, value];
}

export function tasksArgs(probe: string, durationMs: number): string[] {
  return ['tasks', ...probeArgs(probe), '--duration', String(Math.round(durationMs))];
}

export function statusArgs(probe: string): string[] {
  return ['status', ...probeArgs(probe)];
}

export type FailureKind = 'no-session' | 'several-sessions' | 'not-found' | 'timeout' | 'failed';

export interface Failure {
  kind: FailureKind;
  message: string;
  /** Probe selectors named in a several-sessions error, as `VID:PID[:SERIAL]`. */
  probes?: string[];
}

/** The message of an anyhow error printed to stderr, without the `Error: ` prefix. */
export function errorMessage(stderr: string): string {
  const text = stderr.trim();
  const lines = text.split(/\r?\n/);
  const index = lines.findIndex((l) => l.startsWith('Error: '));
  if (index < 0) {
    // clap prints `error: ...` followed by usage lines.
    if (text.startsWith('error: ')) {
      const first = lines[0].slice('error: '.length).trim();
      return /unrecognized subcommand 'inspect'/.test(first)
        ? 'This tuning-studio has no `inspect` command; install a newer CLI'
        : first;
    }
    return text;
  }
  const first = lines[index].slice('Error: '.length).trim();
  // anyhow prints causes under "Caused by:", indented and numbered.
  const causes = lines
    .slice(index + 1)
    .map((l) => l.trim().replace(/^\d+:\s*/, ''))
    .filter((l) => l !== '' && l !== 'Caused by:');
  return [first, ...causes].join(': ');
}

/** Sort a failed command into the cases the UI handles differently. */
export function classifyFailure(input: {
  stderr: string;
  code?: string | number | null;
  killed?: boolean;
  executable: string;
}): Failure {
  if (input.code === 'ENOENT') {
    return {
      kind: 'not-found',
      message: `${input.executable} was not found. Install the CLI or set tuningStudio.executable`,
    };
  }
  if (input.killed) {
    return { kind: 'timeout', message: 'the command did not finish in time' };
  }
  const message = errorMessage(input.stderr) || `exited with ${input.code ?? 'an error'}`;
  if (message.includes('--chip is needed when no `run` or `attach` is running')) {
    return { kind: 'no-session', message: 'No running session' };
  }
  const several = /sessions are running for several probes \(([^)]*)\)/.exec(message);
  if (several) {
    return {
      kind: 'several-sessions',
      message,
      probes: several[1]
        .split(',')
        .map((p) => p.trim())
        .filter((p) => p !== ''),
    };
  }
  return { kind: 'failed', message };
}

export interface ListedProbe {
  selector: string;
  identifier: string;
}

/** Parse `tuning-studio list`: `ef1a:74e5:SERIAL  identifier` per probe. */
export function parseProbeList(stdout: string): ListedProbe[] {
  const probes: ListedProbe[] = [];
  for (const line of stdout.split(/\r?\n/)) {
    const match = /^([0-9a-fA-F]{4}:[0-9a-fA-F]{4}(?::\S+)?)\s+(.*)$/.exec(line.trim());
    if (match) {
      probes.push({ selector: match[1], identifier: match[2].trim() });
    }
  }
  return probes;
}
