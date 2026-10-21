import { randomBytes } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import * as vscode from "vscode";
import { StudioSession, STORAGE_KEY, type FromPage } from "./session";

export const VIEW_TYPE = "tuningStudio";

export class StudioPanel {
  static current: StudioPanel | undefined;
  private disposed = false;
  private readonly disposables: vscode.Disposable[] = [];

  static show(context: vscode.ExtensionContext, session: StudioSession) {
    if (StudioPanel.current) {
      StudioPanel.current.panel.reveal();
      return;
    }
    const panel = vscode.window.createWebviewPanel(VIEW_TYPE, "Tuning Scope", vscode.ViewColumn.Active, {
      retainContextWhenHidden: true,
    });
    StudioPanel.current = new StudioPanel(panel, context, session);
  }

  static revive(panel: vscode.WebviewPanel, context: vscode.ExtensionContext, session: StudioSession) {
    StudioPanel.current?.panel.dispose();
    StudioPanel.current = new StudioPanel(panel, context, session);
  }

  private readonly panel: vscode.WebviewPanel;
  private readonly context: vscode.ExtensionContext;

  private constructor(panel: vscode.WebviewPanel, context: vscode.ExtensionContext, session: StudioSession) {
    this.panel = panel;
    this.context = context;
    const media = vscode.Uri.joinPath(context.extensionUri, "media");
    panel.webview.options = { enableScripts: true, localResourceRoots: [media] };
    this.disposables.push(
      session.onMessage((message) => { void panel.webview.postMessage(message); }),
      panel.webview.onDidReceiveMessage((m: FromPage) => session.receive(m, (message) => { if (!this.disposed) void panel.webview.postMessage(message); })),
      panel.onDidDispose(() => this.dispose()),
    );
    panel.webview.html = this.html(media);
  }

  private html(media: vscode.Uri): string {
    const index = join(media.fsPath, "index.html");
    if (!existsSync(index)) {
      return `<!doctype html><body style="font-family: var(--vscode-font-family); padding: 1em">
        <p>The Tuning Studio page is not built. Run <code>npm run build:webview</code> in the repository, then reopen.</p></body>`;
    }
    const webview = this.panel.webview;
    const nonce = randomBytes(16).toString("base64");
    const csp = [
      "default-src 'none'",
      `img-src ${webview.cspSource} data:`,
      `font-src ${webview.cspSource}`,
      `style-src ${webview.cspSource} 'unsafe-inline'`,
      `script-src 'nonce-${nonce}'`,
    ].join("; ");
    const storage = this.context.workspaceState.get<Record<string, string>>(STORAGE_KEY) ?? {};
    // A JSON script is data, not code; `<` is escaped so no value can close the tag
    const boot = JSON.stringify({ storage }).replace(/</g, "\\u003c");
    return readFileSync(index, "utf8")
      .replace(/(src|href)="\.\/([^"]+)"/g, (_, attr: string, path: string) => {
        return `${attr}="${webview.asWebviewUri(vscode.Uri.joinPath(media, path))}"`;
      })
      .replace(/<script /g, `<script nonce="${nonce}" `)
      .replace(
        "<head>",
        `<head>\n    <meta http-equiv="Content-Security-Policy" content="${csp}" />` +
          `\n    <script id="tuning-studio-boot" type="application/json">${boot}</script>`,
      );
  }

  private dispose() {
    this.disposed = true;
    if (StudioPanel.current === this) StudioPanel.current = undefined;
    this.disposables.forEach((d) => d.dispose());
  }
}
