import type { CliResult } from './cli';
import { isObject } from './launchConfig';

export interface WatchSuggestion { path: string; label: string; type: string }
export interface WatchSuggestions { items: WatchSuggestion[]; message?: string }

export function parseCompletions(stdout: string): WatchSuggestion[] {
  const data: unknown = JSON.parse(stdout);
  if (!isObject(data) || data.version !== 1 || !Array.isArray(data.items) || data.items.length > 50) {
    throw new Error('Unsupported watch suggestions; update the CLI');
  }
  return data.items.map((item: unknown) => {
    if (!isObject(item) || typeof item.path !== 'string' || !item.path || item.path.length > 1024
      || typeof item.label !== 'string' || typeof item.type !== 'string') {
      throw new Error('Invalid watch suggestion from the CLI');
    }
    return { path: item.path, label: item.label, type: item.type };
  });
}

/** Older running sessions cannot gain commands when their executable is rebuilt. */
export async function watchSuggestions(
  run: (args: string[]) => Promise<CliResult>,
  query: string,
  target: { live: boolean; probe: string; elf?: string },
): Promise<WatchSuggestions> {
  if (!target.live && !target.elf) return { items: [], message: 'Select an ELF for suggestions' };
  const offline = () => run(['complete', '--elf', target.elf!, '--', query]);
  let result = target.live
    ? await run(['complete', ...(target.probe ? ['--probe', target.probe] : []), '--', query])
    : await offline();
  const unsupported = (message: string) => /(?:unrecognized|unknown|unsupported) (?:sub)?command.*[\x27`"]complete[\x27`"]|no [\x27`"]complete[\x27`"] command/i.test(message);
  if (!result.ok && target.live && target.elf && (unsupported(result.failure.message) || result.failure.kind === 'no-session')) {
    result = await offline();
  }
  if (!result.ok) return { items: [], message: unsupported(result.failure.message)
    ? 'Rebuild the CLI, check tuningStudio.executable, then restart Run for suggestions'
    : result.failure.message };
  return { items: parseCompletions(result.stdout) };
}
