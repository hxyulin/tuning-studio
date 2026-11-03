import * as vscode from 'vscode';
import { Cli, CliResult } from './cli';
import { inspectArgs } from './cliText';
import { InspectValue, flattenValues, matchLocations, parseInspect } from './inspect';
import { SessionController } from './session';
import { pollInterval, probe } from './settings';

const WATCHES_KEY = 'tuningStudio.watches';

export interface PollSample {
  /** Host time in seconds when the poll finished. */
  time: number;
  /** Plottable numbers by path; a path with no usable number is absent. */
  numbers: Map<string, number>;
  /** False when every inspect call in the poll failed. */
  ok: boolean;
}

/**
 * The values Live Watch shows: every value the firmware declares, plus the
 * locations the user added. Polls `inspect` with at most one call in flight.
 */
export class LiveWatch implements vscode.Disposable {
  declared: InspectValue[] = [];
  declaredError?: string;
  watched: string[];
  watchedValues = new Map<string, InspectValue[]>();
  watchedError?: string;
  lastSuccess?: Date;
  paused = false;

  private blocked = 0;
  private wasActive = false;
  private running = false;
  private timer?: NodeJS.Timeout;
  private readonly startTime = performance.now();
  private readonly changed = new vscode.EventEmitter<void>();
  private readonly sampled = new vscode.EventEmitter<PollSample>();
  private readonly disposables: vscode.Disposable[] = [this.changed, this.sampled];

  readonly onDidChange = this.changed.event;
  readonly onDidSample = this.sampled.event;

  constructor(
    private readonly cli: Cli,
    private readonly session: SessionController,
    private readonly state: vscode.Memento,
  ) {
    const saved = state.get<unknown>(WATCHES_KEY, []);
    this.watched = Array.isArray(saved) ? saved.filter((w): w is string => typeof w === 'string') : [];
    this.disposables.push(session.onDidChange(() => this.reschedule()));
  }

  /** Host seconds since the extension started; the plot's time axis. */
  now(): number {
    return (performance.now() - this.startTime) / 1000;
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    void vscode.commands.executeCommand('setContext', 'tuningStudio.pollingPaused', paused);
    this.changed.fire();
    this.reschedule();
  }

  /** Stop polling while another command needs the session, such as `tasks`. */
  async block<T>(work: () => Promise<T>): Promise<T> {
    this.blocked++;
    this.reschedule();
    try {
      return await work();
    } finally {
      this.blocked--;
      this.reschedule();
    }
  }

  get active(): boolean {
    return this.session.isReady && !this.paused && this.blocked === 0;
  }

  async addWatch(location: string): Promise<void> {
    if (!this.watched.includes(location)) {
      this.watched = [...this.watched, location];
      await this.saveWatches();
    }
  }

  async replaceWatch(old: string, location: string): Promise<void> {
    this.watched = this.watched.map((w) => (w === old ? location : w)).filter((w, i, all) => all.indexOf(w) === i);
    this.watchedValues.delete(old);
    await this.saveWatches();
  }

  async removeWatch(location: string): Promise<void> {
    this.watched = this.watched.filter((w) => w !== location);
    this.watchedValues.delete(location);
    await this.saveWatches();
  }

  private async saveWatches(): Promise<void> {
    await this.state.update(WATCHES_KEY, this.watched);
    this.changed.fire();
    this.pollSoon();
  }

  /** The latest value read for a path, from either group. */
  lookup(path: string): InspectValue | undefined {
    for (const values of this.watchedValues.values()) {
      const found = flattenValues(values).find((v) => v.path === path);
      if (found) {
        return found;
      }
    }
    return flattenValues(this.declared).find((v) => v.path === path);
  }

  /** Start or stop polling to match `active`, keeping a poll already scheduled. */
  private reschedule(): void {
    const active = this.active;
    // Polling stopped: end every plotted line here, so it does not bridge the pause.
    if (this.wasActive && !active) {
      this.sampled.fire({ time: this.now(), numbers: new Map(), ok: false });
    }
    this.wasActive = active;
    if (!active) {
      if (this.timer) {
        clearTimeout(this.timer);
        this.timer = undefined;
      }
      return;
    }
    if (!this.timer && !this.running) {
      this.timer = setTimeout(() => void this.poll(), 0);
    }
  }

  /** Poll as soon as the current poll, if any, has finished. */
  private pollSoon(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    this.reschedule();
  }

  private async poll(): Promise<void> {
    this.timer = undefined;
    if (this.running || !this.active) {
      return;
    }
    this.running = true;
    const started = performance.now();
    // Only values read in this poll are plotted; older ones would hide a gap.
    const fresh: InspectValue[] = [];
    let ok = false;
    try {
      const declared = await this.cli.run(inspectArgs(probe()), { timeoutMs: 5000 });
      ok = this.apply(
        declared,
        (values) => {
          this.declared = values;
          this.declaredError = undefined;
          fresh.push(...values);
        },
        (message) => (this.declaredError = message),
      );

      const locations = [...this.watched];
      if (locations.length > 0 && this.session.isReady) {
        const watched = await this.cli.run(inspectArgs(probe(), locations), { timeoutMs: 5000 });
        const watchedOk = this.apply(
          watched,
          (values) => {
            this.watchedValues = matchLocations(locations, values);
            this.watchedError = undefined;
            fresh.push(...values);
          },
          (message) => (this.watchedError = message),
        );
        ok ||= watchedOk;
      }
      if (ok) {
        this.lastSuccess = new Date();
      }
    } finally {
      this.running = false;
    }
    const numbers = new Map<string, number>();
    for (const value of flattenValues(fresh)) {
      if (value.number !== undefined && value.error === undefined) {
        numbers.set(value.path, value.number);
      }
    }
    this.sampled.fire({ time: this.now(), numbers, ok });
    this.changed.fire();
    if (this.active) {
      const wait = Math.max(0, pollInterval() - (performance.now() - started));
      this.timer = setTimeout(() => void this.poll(), wait);
    }
  }

  /**
   * Take an inspect result. Its JSON is used even when the command exited
   * non-zero, since per-value errors are reported inside it.
   */
  private apply(
    result: CliResult,
    onValues: (values: InspectValue[]) => void,
    onError: (message: string) => void,
  ): boolean {
    if (result.stdout.trim().startsWith('{')) {
      try {
        onValues(parseInspect(result.stdout).values);
        return true;
      } catch (error) {
        onError(error instanceof Error ? error.message : String(error));
        return false;
      }
    }
    if (result.ok) {
      onError('inspect printed nothing');
      return false;
    }
    if (result.failure.kind === 'no-session') {
      this.session.markLost();
    } else {
      this.session.markFailure(result.failure);
    }
    onError(result.failure.message);
    return false;
  }

  dispose(): void {
    if (this.timer) {
      clearTimeout(this.timer);
    }
    this.disposables.forEach((d) => d.dispose());
  }
}
