import * as vscode from 'vscode';
import { Cli } from './cli';
import { Failure, statusArgs } from './cliText';
import { probe } from './settings';

export type SessionState =
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'ready'; status: string }
  | { kind: 'error'; failure: Failure };

const IDLE_CHECK_MS = 2000;

/** Observes a CLI session. Building, starting and stopping belong to its terminal. */
export class SessionController implements vscode.Disposable {
  private stateValue: SessionState = { kind: 'unknown' };
  private checkTimer?: NodeJS.Timeout;
  private inflight?: Promise<SessionState>;
  private monitorWanted = false;
  private disposed = false;
  private readonly emitter = new vscode.EventEmitter<SessionState>();
  readonly onDidChange = this.emitter.event;

  constructor(private readonly cli: Cli) {}

  get state(): SessionState { return this.stateValue; }
  get isReady(): boolean { return this.stateValue.kind === 'ready'; }

  private setState(state: SessionState): void {
    if (this.disposed) return;
    this.stateValue = state;
    this.emitter.fire(state);
    this.scheduleCheck();
  }

  /** Check for a session now and keep checking while a tuning view is visible. */
  setMonitoring(wanted: boolean): void {
    this.monitorWanted = wanted;
    this.scheduleCheck();
  }

  private scheduleCheck(): void {
    if (this.checkTimer) {
      clearTimeout(this.checkTimer);
      this.checkTimer = undefined;
    }
    const kind = this.stateValue.kind;
    if (!this.monitorWanted) {
      return;
    }
    const delay = kind === 'unknown' ? 0 : IDLE_CHECK_MS;
    this.checkTimer = setTimeout(() => void this.check(), delay);
  }

  /** Run `status` once and update the state from its answer. Concurrent calls share one run. */
  check(): Promise<SessionState> {
    this.inflight ??= this.checkOnce().finally(() => (this.inflight = undefined));
    return this.inflight;
  }

  private async checkOnce(): Promise<SessionState> {
    const result = await this.cli.run(statusArgs(probe()), { timeoutMs: 5000 });
    if (result.ok) {
      const status = result.stdout.split(/\r?\n/)[0]?.trim() || 'running';
      this.setState({ kind: 'ready', status });
    } else if (result.failure.kind === 'no-session') {
      this.setState({ kind: 'none' });
    } else {
      this.setState({ kind: 'error', failure: result.failure });
    }
    return this.stateValue;
  }

  /** A command found no session, so the one shown is gone. */
  markLost(): void {
    if (this.stateValue.kind === 'ready') {
      this.setState({ kind: 'none' });
    }
  }

  markFailure(failure: Failure): void {
    if (this.stateValue.kind === 'ready' && failure.kind === 'several-sessions') {
      this.setState({ kind: 'error', failure });
    }
  }

  dispose(): void {
    this.disposed = true;
    if (this.checkTimer) clearTimeout(this.checkTimer);
    this.emitter.dispose();
  }
}
