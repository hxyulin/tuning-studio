// The probe belongs to one process at a time. While a probe-rs debug session runs, the studio may
// not take the probe; when one starts while the studio holds it, the studio lets go first so the
// debugger's attach does not fail.

import * as vscode from "vscode";

export const DEBUG_TYPE = "probe-rs-debug";
/** A launch that resolved but never started (a failed preLaunchTask) stops blocking after this */
const PENDING_TIMEOUT_MS = 120_000;

/** Something that may hold the probe: a studio panel */
export interface ProbeHolder {
  holdsProbe(): boolean;
  /** Stop sampling and let go of the probe, because debug session `name` wants it */
  releaseProbe(name: string): Promise<void>;
  /** The set of running debug sessions changed */
  debugSessionsChanged(): void;
}

export class ProbeGuard implements vscode.Disposable {
  /** Running probe-rs sessions, by id */
  private readonly sessions = new Map<string, string>();
  /** Launches resolved but not yet started, by name */
  private readonly pending = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly holders = new Set<ProbeHolder>();
  private readonly disposables: vscode.Disposable[] = [];

  private readonly log: vscode.OutputChannel;

  constructor(log: vscode.OutputChannel) {
    this.log = log;
    const active = vscode.debug.activeDebugSession;
    if (active?.type === DEBUG_TYPE) this.sessions.set(active.id, active.name);

    this.disposables.push(
      // Resolution runs before the adapter starts and VS Code waits for it: the probe is free by then
      vscode.debug.registerDebugConfigurationProvider(DEBUG_TYPE, {
        resolveDebugConfigurationWithSubstitutedVariables: async (_folder, config) => {
          const name = config.name || DEBUG_TYPE;
          this.markPending(name);
          await this.release(name);
          return config;
        },
      }),
      // Fallback for sessions started without resolution (a restart, another extension's launch)
      vscode.debug.registerDebugAdapterTrackerFactory(DEBUG_TYPE, {
        createDebugAdapterTracker: (session) => {
          this.started(session);
          void this.release(session.name);
          return undefined;
        },
      }),
      vscode.debug.onDidStartDebugSession((session) => {
        if (session.type !== DEBUG_TYPE) return;
        this.started(session);
        void this.release(session.name);
      }),
      vscode.debug.onDidTerminateDebugSession((session) => {
        if (!this.sessions.delete(session.id)) return;
        this.log.appendLine(`debug session "${session.name}" ended`);
        this.changed();
      }),
    );
  }

  /** The debug session using the probe, if any */
  blockedBy(): string | null {
    for (const name of this.sessions.values()) return name;
    for (const name of this.pending.keys()) return name;
    return null;
  }

  add(holder: ProbeHolder): vscode.Disposable {
    this.holders.add(holder);
    return new vscode.Disposable(() => this.holders.delete(holder));
  }

  dispose() {
    this.disposables.forEach((d) => d.dispose());
    this.pending.forEach((timer) => clearTimeout(timer));
  }

  private started(session: vscode.DebugSession) {
    const known = this.sessions.has(session.id);
    this.sessions.set(session.id, session.name);
    const timer = this.pending.get(session.name);
    if (timer) clearTimeout(timer);
    this.pending.delete(session.name);
    if (!known) {
      this.log.appendLine(`debug session "${session.name}" started`);
      this.changed();
    }
  }

  private markPending(name: string) {
    const old = this.pending.get(name);
    if (old) clearTimeout(old);
    this.pending.set(
      name,
      setTimeout(() => {
        this.pending.delete(name);
        this.changed();
      }, PENDING_TIMEOUT_MS),
    );
    this.changed();
  }

  private async release(name: string) {
    const holding = [...this.holders].filter((h) => h.holdsProbe());
    if (!holding.length) return;
    this.log.appendLine(`releasing the probe for debug session "${name}"`);
    await Promise.all(holding.map((h) => h.releaseProbe(name).catch(() => {})));
  }

  private changed() {
    this.holders.forEach((h) => h.debugSessionsChanged());
  }
}
