// Just enough of the `vscode` API to run the extension headlessly in scripts/harness.ts.

import { join } from "node:path";

type Listener<T> = (value: T) => unknown;

export class Disposable {
  private readonly fn: () => void;
  constructor(fn: () => void) {
    this.fn = fn;
  }
  dispose() {
    this.fn();
  }
}

export class Uri {
  readonly fsPath: string;
  constructor(fsPath: string) {
    this.fsPath = fsPath;
  }
  static file(path: string) {
    return new Uri(path);
  }
  static joinPath(base: Uri, ...parts: string[]) {
    return new Uri(join(base.fsPath, ...parts));
  }
  toString() {
    const path = this.fsPath.replace(/\\/g, "/");
    return `https://file+.vscode-resource.test${path.startsWith("/") ? "" : "/"}${path}`;
  }
}

export const ViewColumn = { Active: -1 } as const;

function emitter<T>() {
  const listeners = new Set<Listener<T>>();
  const event = (l: Listener<T>) => {
    listeners.add(l);
    return new Disposable(() => listeners.delete(l));
  };
  return { event, fire: (v: T) => listeners.forEach((l) => l(v)) };
}

/** What the harness inspects and drives */
export const harness = {
  commands: new Map<string, (...args: any[]) => unknown>(),
  configProviders: [] as { resolveDebugConfigurationWithSubstitutedVariables?: (f: unknown, c: any) => Promise<any> }[],
  started: emitter<any>(),
  terminated: emitter<any>(),
  panels: [] as any[],
  trees: new Map<string, any>(),
  statusBars: [] as any[],
  quickPicks: [] as (number | undefined)[],
  documents: [] as string[],
  inputValues: [] as (string | undefined)[],
  settings: {} as Record<string, unknown>,
  info: [] as string[],
  errors: [] as string[],
  log: [] as string[],
  launch: [] as unknown[],
  /** What the next save dialog answers */
  savePath: undefined as string | undefined,
  /** Commands run through `commands.executeCommand` */
  executed: [] as { name: string; args: unknown[] }[],
};

export const commands = {
  registerCommand(name: string, fn: (...args: any[]) => unknown) {
    harness.commands.set(name, fn);
    return new Disposable(() => harness.commands.delete(name));
  },
  async executeCommand(name: string, ...args: unknown[]) {
    harness.executed.push({ name, args });
    return harness.commands.get(name)?.(...args);
  },
};

export class EventEmitter<T> {
  private readonly emitter = emitter<T>();
  readonly event = this.emitter.event;
  fire(value: T) { this.emitter.fire(value); }
  dispose() {}
}
export const StatusBarAlignment = { Left: 1, Right: 2 } as const;
export const TreeItemCollapsibleState = { None: 0, Collapsed: 1, Expanded: 2 } as const;
export class TreeItem {
  label: string;
  collapsibleState: number;
  constructor(label: string, collapsibleState: number = TreeItemCollapsibleState.None) {
    this.label = label;
    this.collapsibleState = collapsibleState;
  }
}
export class Position {
  line: number;
  character: number;
  constructor(line: number, character: number) {
    this.line = line;
    this.character = character;
  }
}
export class Range {
  start: Position;
  end: Position;
  constructor(start: Position, end: Position) {
    this.start = start;
    this.end = end;
  }
}
export class Selection extends Range {}
export class ThemeIcon {
  id: string;
  constructor(id: string) {
    this.id = id;
  }
}

export const window = {
  showTextDocument: async () => ({ selection: undefined as unknown, revealRange() {} }),
  registerTreeDataProvider: (id: string, provider: any) => {
    harness.trees.set(id, provider);
    return new Disposable(() => harness.trees.delete(id));
  },
  createStatusBarItem: () => {
    const status = { text: "", show() {}, dispose() {} };
    harness.statusBars.push(status);
    return status;
  },
  showQuickPick: async (items: any[]) => {
    const index = harness.quickPicks.shift();
    return index === undefined ? undefined : items[index];
  },
  showInputBox: async () => harness.inputValues.shift(),
  createOutputChannel: () => ({ appendLine: (l: string) => harness.log.push(l), show() {}, dispose() {} }),
  showErrorMessage: async (m: string) => void harness.errors.push(m),
  showInformationMessage: async (m: string) => void harness.info.push(m),
  showOpenDialog: async () => undefined,
  showSaveDialog: async () => (harness.savePath ? Uri.file(harness.savePath) : undefined),
  registerWebviewPanelSerializer: () => new Disposable(() => {}),
  createWebviewPanel(_type: string, _title: string, _column: unknown, options: unknown) {
    const fromPage = emitter<any>();
    const disposed = emitter<void>();
    const panel = {
      createOptions: options,
      toPage: [] as any[],
      send: (m: any) => fromPage.fire(m),
      webview: {
        options: {},
        html: "",
        cspSource: "https://file+.vscode-resource.test",
        asWebviewUri: (u: Uri) => u.toString(),
        postMessage: async (m: any) => {
          panel.toPage.push(m);
          return true;
        },
        onDidReceiveMessage: fromPage.event,
      },
      reveal() {},
      onDidDispose: disposed.event,
      dispose: () => disposed.fire(),
    };
    harness.panels.push(panel);
    return panel;
  },
};

export const workspace = {
  openTextDocument: async (uri: Uri) => { harness.documents.push(uri.fsPath); return {}; },
  workspaceFolders: undefined as unknown,
  getConfiguration(section: string) {
    return {
      get: (key: string, fallback?: unknown) => (section === "launch" && key === "configurations" ? harness.launch : harness.settings[`${section}.${key}`] ?? fallback),
    };
  },
};

export const debug = {
  activeDebugSession: undefined,
  registerDebugConfigurationProvider(_type: string, provider: any) {
    harness.configProviders.push(provider);
    return new Disposable(() => {});
  },
  registerDebugAdapterTrackerFactory: () => new Disposable(() => {}),
  onDidStartDebugSession: harness.started.event,
  onDidTerminateDebugSession: harness.terminated.event,
};
