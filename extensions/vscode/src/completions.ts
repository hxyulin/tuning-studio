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
