import { randomBytes } from 'node:crypto';
import * as vscode from 'vscode';
import { PollSample } from './liveWatch';
import { parseWebviewMessage, ToWebview } from './messages';
import { Series } from './series';
import { plotCapacity, plotWindow } from './settings';

const PLOTTED_KEY = 'tuningStudio.plotted';
export const MAX_SERIES = 8;
export interface PlotConfig { id: string; title: string; paths: string[]; hidden?: string[] }

/**
 * The plotted values and their samples. Samples live here, not in the
 * webview, so hiding the panel loses nothing.
 */
export class PlotModel {
  private readonly series = new Map<string, Series>();
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeSelection = this.changed.event;
  plots: PlotConfig[] = [];
  choices: string[] = [];
  private frozenValue = false;
  private latest = 0;
  frozenEnd = 0;
  windowSeconds = plotWindow();
  capacity = plotCapacity();

  constructor(private readonly state: vscode.Memento) {
    const layout = state.get<{ plots?: PlotConfig[]; columns?: number }>('tuningStudio.layout', {});
    const saved = state.get<unknown>(PLOTTED_KEY, []);
    if (Array.isArray(saved)) {
      for (const path of saved) {
        if (typeof path === 'string' && this.series.size < MAX_SERIES) {
          this.series.set(path, new Series(this.capacity));
        }
      }
    }
    if (Array.isArray(layout.plots)) {
      // Fold older multi-plot workspaces into one plot without losing values.
      const valid = layout.plots.filter(p => p && typeof p.id === 'string' && typeof p.title === 'string' && Array.isArray(p.paths));
      if (valid.length) {
        const paths = [...new Set(valid.flatMap(p => p.paths))].filter(path => this.series.has(path)).slice(0, MAX_SERIES);
        this.plots = [{ id: valid[0].id, title: valid[0].title.slice(0, 80), paths,
          hidden: paths.filter(path => valid.every(p => !p.paths.includes(path) || p.hidden?.includes(path))) }];
      }
    } else if (this.paths.length) {
      this.plots = [{ id: 'default', title: 'Plot', paths: this.paths.slice(0, MAX_SERIES), hidden: [] }];
    }
    const used = new Set(this.plots.flatMap(p => p.paths));
    for (const path of this.series.keys()) if (!used.has(path)) this.series.delete(path);
  }

  private async saveLayout(): Promise<void> {
    const used = new Set(this.plots.flatMap(p => p.paths));
    for (const path of this.series.keys()) if (!used.has(path)) this.series.delete(path);
    await this.state.update('tuningStudio.layout', { plots: this.plots });
    await this.state.update(PLOTTED_KEY, this.paths);
    this.changed.fire();
  }

  async layout(action: string, id: string, path: string, text: string): Promise<void> {
    if (action === 'seriesAdd' && !this.plots.length && this.choices.includes(path)) {
      this.plots.push({ id: 'default', title: 'Plot', paths: [], hidden: [] });
    }
    const plot = this.plots.find(p => p.id === id) ?? (id === '' ? this.plots[0] : undefined);
    if (!plot) return;
    if (action === 'remove') this.plots = [];
    if (action === 'rename' && text.trim()) plot.title = text.trim().slice(0, 80);
    if (action === 'seriesAdd' && this.choices.includes(path) && !plot.paths.includes(path) && plot.paths.length < MAX_SERIES) {
      plot.paths.push(path);
      this.series.set(path, new Series(this.capacity));
    }
    if (action === 'seriesToggle' && plot.paths.includes(path)) {
      const hidden = new Set(plot.hidden ?? []);
      if (hidden.has(path)) hidden.delete(path); else hidden.add(path);
      plot.hidden = [...hidden];
    }
    if (action === 'seriesRemove') {
      plot.paths = plot.paths.filter(p => p !== path);
      plot.hidden = plot.hidden?.filter(p => p !== path);
    }
    await this.saveLayout();
  }

  get paths(): string[] {
    return [...this.series.keys()];
  }

  get frozen(): boolean {
    return this.frozenValue;
  }

  set frozen(value: boolean) {
    if (value && !this.frozenValue) {
      this.frozenEnd = this.latest;
    }
    this.frozenValue = value;
  }

  has(path: string): boolean {
    return this.series.has(path);
  }

  /** Add or remove a path. Returns false when the plot is full. */
  async set(path: string, plotted: boolean): Promise<boolean> {
    if (plotted === this.series.has(path)) {
      return true;
    }
    if (plotted) {
      if (this.series.size >= MAX_SERIES) {
        return false;
      }
      this.series.set(path, new Series(this.capacity));
    } else {
      this.series.delete(path);
    }
    if (plotted) {
      if (!this.plots.length) this.plots.push({ id: 'default', title: 'Plot', paths: [], hidden: [] });
      this.plots[0].paths.push(path);
    } else this.plots.forEach(p => p.paths = p.paths.filter(v => v !== path));
    await this.saveLayout();
    return true;
  }

  /** Record one poll. A path with no number in it gets a gap. */
  record(sample: PollSample): [string, number | null][] {
    this.latest = sample.time;
    const values: [string, number | null][] = [];
    for (const [path, series] of this.series) {
      const value = sample.numbers.get(path);
      series.push(sample.time, value);
      values.push([path, value ?? null]);
    }
    return values;
  }

  /** Break every line, for a pause in polling such as a tasks sample. */
  gap(time: number): [string, number | null][] {
    return this.record({ time, numbers: new Map(), ok: false });
  }

  clear(): void {
    this.series.forEach((s) => s.clear());
  }

  resize(capacity: number): void {
    if (capacity === this.capacity) {
      return;
    }
    this.capacity = capacity;
    for (const path of this.series.keys()) {
      this.series.set(path, new Series(capacity));
    }
  }

  init(): ToWebview {
    return {
      type: 'init',
      plots: this.plots,
      choices: this.choices,
      windowSeconds: this.windowSeconds,
      capacity: this.capacity,
      frozen: this.frozen,
      frozenEnd: this.frozenEnd,
      series: [...this.series].map(([path, s]) => ({ path, ...s.snapshot() })),
    };
  }
}

export class PlotViewProvider implements vscode.WebviewViewProvider, vscode.Disposable {
  private view?: vscode.WebviewView;
  private status = '';
  private readonly disposables: vscode.Disposable[] = [];
  private readonly visibility = new vscode.EventEmitter<boolean>();
  readonly onDidChangeVisibility = this.visibility.event;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly model: PlotModel,
    private readonly unplot: (path: string) => void,
  ) {
    this.disposables.push(model.onDidChangeSelection(() => this.reset()));
  }

  get visible(): boolean {
    return this.view?.visible ?? false;
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    const media = vscode.Uri.joinPath(this.extensionUri, 'media');
    view.webview.options = { enableScripts: true, localResourceRoots: [media] };
    view.webview.html = this.html(view.webview, media);
    this.disposables.push(
      view.webview.onDidReceiveMessage((raw: unknown) => {
        const message = parseWebviewMessage(raw);
        if (!message) {
          return;
        }
        if (message.type === 'ready') {
          this.post(this.model.init());
          this.post({ type: 'status', text: this.status });
        } else if (message.type === 'layout') {
          void this.model.layout(message.action, message.id, message.path, message.text);
        } else if (message.type === 'unplot' && this.model.has(message.path)) {
          this.unplot(message.path);
        }
      }),
      view.onDidChangeVisibility(() => this.visibility.fire(view.visible)),
      view.onDidDispose(() => {
        this.view = undefined;
        this.visibility.fire(false);
      }),
    );
    this.visibility.fire(view.visible);
  }

  append(time: number, values: [string, number | null][]): void {
    if (values.length > 0) {
      this.post({ type: 'append', t: time, values });
    }
  }

  setStatus(text: string): void {
    if (text !== this.status) {
      this.status = text;
      this.post({ type: 'status', text });
    }
  }

  refreshView(): void {
    this.post({ type: 'view', windowSeconds: this.model.windowSeconds, frozen: this.model.frozen, frozenEnd: this.model.frozenEnd });
  }

  reset(): void {
    this.post(this.model.init());
  }

  private post(message: ToWebview): void {
    if (this.view?.visible) {
      void this.view.webview.postMessage(message);
    }
  }

  private html(webview: vscode.Webview, media: vscode.Uri): string {
    const nonce = randomBytes(16).toString('base64');
    const script = webview.asWebviewUri(vscode.Uri.joinPath(media, 'plot.js'));
    const style = webview.asWebviewUri(vscode.Uri.joinPath(media, 'plot.css'));
    const csp = [
      "default-src 'none'",
      `style-src ${webview.cspSource}`,
      `script-src 'nonce-${nonce}'`,
    ].join('; ');
    return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<link rel="stylesheet" href="${style}">
<title>Plot</title>
</head>
<body>
<main id="plot">
  <div id="lanes"></div>
  <footer id="footer">
    <span id="status"></span>

  </footer>
</main>
<script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }

  dispose(): void {
    this.visibility.dispose();
    this.disposables.forEach((d) => d.dispose());
  }
}
