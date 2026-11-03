import { randomBytes } from 'node:crypto';
import * as vscode from 'vscode';
import { editReason, InspectValue } from './inspect';
import { LiveWatch } from './liveWatch';

export interface WriteResult { ok: boolean; text: string }

/** Live values with row-local edit state, independent of incoming polls. */
export class WatchPanel implements vscode.WebviewViewProvider, vscode.Disposable {
  private view?: vscode.WebviewView;
  /** Paths with a write in flight; a second write for the same path is dropped. */
  private readonly writing = new Set<string>();
  private message = '';
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeVisibility = this.changed.event;
  private readonly disposables: vscode.Disposable[] = [this.changed];
  constructor(
    private readonly uri: vscode.Uri,
    private readonly watch: LiveWatch,
    private readonly write: (value: InspectValue, input: string) => Promise<WriteResult>,
    private readonly suggest: (path: string) => void,
  ) {}
  get visible(): boolean { return this.view?.visible ?? false; }
  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    const media = vscode.Uri.joinPath(this.uri, 'media');
    view.webview.options = { enableScripts: true, localResourceRoots: [media] };
    const nonce = randomBytes(16).toString('base64');
    const asset = (file: string) => view.webview.asWebviewUri(vscode.Uri.joinPath(media, file));
    view.webview.html = `<!doctype html><html><head><meta charset="UTF-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${view.webview.cspSource}; script-src 'nonce-${nonce}'"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="${asset('watch.css')}"></head><body><p id="status" role="status" hidden></p><div id="values" role="tree" aria-label="Live values"></div><p id="empty" hidden>No values yet. Run firmware, or add a symbol or address:type below.</p><form id="add"><input id="location" aria-label="Add watch: symbol path or address:type" placeholder="Add watch: symbol or address:type" spellcheck="false" autocomplete="off"></form><div id="announce" class="sr-only" aria-live="polite"></div><script nonce="${nonce}" src="${asset('watch.js')}"></script></body></html>`;
    this.disposables.push(view.onDidChangeVisibility(() => { this.refresh(); this.changed.fire(); }), view.onDidDispose(() => { this.view = undefined; this.changed.fire(); }), view.webview.onDidReceiveMessage(async (raw: unknown) => {
      if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
      const m = raw as Record<string, unknown>;
      if (m.type === 'ready') { this.refresh(); return; }
      if (typeof m.path !== 'string' || !m.path || m.path.length > 1024) return;
      if (m.type === 'add') { await this.watch.addWatch(m.path); return; }
      if (m.type === 'remove' && this.watch.watched.includes(m.path)) { await this.watch.removeWatch(m.path); return; }
      if (m.type === 'expand' && (m.input === 'true' || m.input === 'false')) {
        if (this.watch.lookup(m.path)?.expandable) {
          const accepted = await this.watch.setExpanded(m.path, m.input === 'true');
          if (accepted === false) void view.webview.postMessage({ type: 'expansionReply', path: m.path, ok: false, text: 'Pointer expansion limit (64). Collapse another pointer first.' });
        }
        return;
      }
      if (m.type === 'write') {
        if (typeof m.input !== 'string' || !m.input.trim() || m.input.length > 1024 || this.writing.has(m.path)) return;
        const path = m.path;
        const value = this.watch.lookup(path);
        let reply: WriteResult;
        if (!value) reply = { ok: false, text: 'This value is no longer available.' };
        else if (!value.writable || value.error) reply = { ok: false, text: editReason(value) };
        else {
          this.writing.add(path);
          try { reply = await this.write(value, m.input.trim()); }
          catch (error) { reply = { ok: false, text: error instanceof Error ? error.message : String(error) }; }
          finally { this.writing.delete(path); }
        }
        void view.webview.postMessage({ type: 'reply', path, ok: reply.ok, text: reply.text });
        return;
      }
      const value = this.watch.lookup(m.path);
      if (m.type === 'copy' && m.input === 'path' && (value || this.watch.watched.includes(m.path))) { await vscode.env.clipboard.writeText(m.path); return; }
      if (!value) return;
      if (m.type === 'suggest' && !value.writable) { this.suggest(value.path); return; }
      if (m.type === 'copy' && m.input === 'value') await vscode.env.clipboard.writeText(value.error ?? value.display);
    }));
    this.refresh(); this.changed.fire();
  }
  refresh(message = this.message): void {
    this.message = message;
    const values = [...this.watch.declared, ...this.watch.watchedValues.values()].flat();
    const unique = [...new Map(values.map(v => [v.path, v])).values()];
    const present = (v: InspectValue): unknown => ({ ...v, reason: editReason(v), children: v.children?.map(present) });
    void this.view?.webview.postMessage({ type: 'values', message, values: unique.map(present), watches: this.watch.watched, errors: this.watch.watched.map(path => ({ path, error: this.watch.watchedError ?? (this.watch.watchedValues.get(path)?.length ? '' : 'Waiting for a value') })) });
  }
  focus(path: string): void { void this.view?.webview.postMessage({ type: 'edit', path }); }
  dispose(): void { this.disposables.forEach(d => d.dispose()); }
}
