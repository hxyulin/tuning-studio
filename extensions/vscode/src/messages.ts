/** Messages between the plot webview and the extension, and their checks. */

export type FromWebview = { type: 'ready' } | { type: 'unplot'; path: string } | { type: 'layout'; action: string; id: string; path: string; text: string };

/** Accept only the messages the plot script sends, with the fields it sets. */
export function parseWebviewMessage(message: unknown): FromWebview | undefined {
  if (typeof message !== 'object' || message === null || Array.isArray(message)) {
    return undefined;
  }
  const m = message as Record<string, unknown>;
  const keys = Object.keys(m);
  if (m.type === 'ready' && keys.length === 1) {
    return { type: 'ready' };
  }
  if (
    m.type === 'unplot' &&
    keys.length === 2 &&
    typeof m.path === 'string' &&
    m.path.length > 0 &&
    m.path.length <= 1024
  ) {
    return { type: 'unplot', path: m.path };
  }
  if (m.type === 'layout' && keys.length === 5 &&
      ['remove', 'rename', 'seriesAdd', 'seriesRemove', 'seriesToggle'].includes(String(m.action)) &&
      ['id', 'path', 'text'].every(k => typeof m[k] === 'string' && (m[k] as string).length <= 1024)) {
    return m as FromWebview;
  }
  return undefined;
}

export interface SeriesSnapshot {
  path: string;
  t: number[];
  v: (number | null)[];
}

export type ToWebview =
  | {
      type: 'init';
      windowSeconds: number;
      capacity: number;
      frozen: boolean;
      frozenEnd: number;
      series: SeriesSnapshot[];
      plots: { id: string; title: string; paths: string[]; hidden?: string[] }[];
      choices: string[];
    }
  | { type: 'append'; t: number; values: [string, number | null][] }
  | { type: 'view'; windowSeconds: number; frozen: boolean; frozenEnd: number }
  | { type: 'status'; text: string };
