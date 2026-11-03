import * as vscode from 'vscode';
import { Cli, CliResult } from './cli';
import { inspectArgs } from './cliText';
import { InspectValue, flattenValues, isDescendant, matchLocations, parseInspect } from './inspect';
import { SessionController } from './session';
import { pollInterval, probe } from './settings';

const WATCHES_KEY = 'tuningStudio.watches';
const EXPANDED_KEY = 'tuningStudio.pointerExpansions';

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

  private readonly expanded = new Set<string>();
  private readonly pointers = new Set<string>();
  private plotted: readonly string[] = [];
  private blocked = 0;
  private wasActive = false;
  private running = false;
  private retryOffset = 0;
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
    const expanded = state.get<unknown>(EXPANDED_KEY, []);
    if (Array.isArray(expanded)) for (const path of expanded.slice(0, 64)) {
      if (typeof path === 'string') { this.expanded.add(path); this.pointers.add(path); }
    }
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

  async setExpanded(path: string, expanded: boolean): Promise<boolean> {
    if (expanded === this.expanded.has(path)) return true;
    const active = this.expansionPaths();
    if (expanded && !active.includes(path) && active.length >= 64) return false;
    this.pointers.add(path);
    if (expanded) this.expanded.add(path); else this.expanded.delete(path);
    await this.state.update(EXPANDED_KEY, [...this.expanded]);
    this.pollSoon();
    return true;
  }

  setPlotted(paths: readonly string[]): void {
    this.plotted = paths;
    // Restore pointer dependencies even before the first poll after a reload.
    for (const path of paths) for (const match of path.matchAll(/\.\*/g)) this.pointers.add(path.slice(0, match.index));
    this.pollSoon();
  }

  private expansionPaths(): string[] {
    const required = [...this.pointers].filter(pointer => this.plotted.some(path => isDescendant(path, pointer)));
    return [...new Set([...required, ...this.expanded])].slice(0, 64);
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
      const declared = await this.cli.run(inspectArgs(probe(), [], this.expansionPaths()), { timeoutMs: 5000 });
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
        const watched = await this.cli.run(inspectArgs(probe(), locations, this.expansionPaths()), { timeoutMs: 5000 });
        this.watchedValues = new Map();
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
        // JSON already carries per-location failures. Retry only a whole-call
        // location failure, never timeouts, missing CLIs, or a lost session.
        if (!watchedOk && !watched.ok && watched.failure.kind === 'failed'
          && !watched.stdout.trim().startsWith('{') && locations.length > 1 && this.active) {
          ok = await this.isolateWatches(locations, fresh) || ok;
        }
      }
      if (ok) {
        this.lastSuccess = new Date();
      }
    } finally {
      this.running = false;
    }
    const numbers = new Map<string, number>();
    for (const value of flattenValues(fresh)) {
      if (value.expandable && this.pointers.size < 1024) this.pointers.add(value.path);
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

  private async isolateWatches(locations: string[], fresh: InspectValue[]): Promise<boolean> {
    const errorValue = (path: string, error: string): InspectValue => ({
      path, type: '', kind: 'raw', writable: false, display: error, error,
    });
    const freshStart = fresh.length;
    for (const location of locations) this.watchedValues.set(location, [errorValue(location, 'Waiting for an individual read')]);
    this.watchedError = undefined;
    let ok = false;
    // Bound recovery work and rotate so long lists do not starve later watches.
    const count = Math.min(locations.length, 8);
    const started = performance.now();
    let attempted = 0;
    for (; attempted < count && this.active; attempted++) {
      if (attempted > 0 && performance.now() - started >= 5000) break;
      const location = locations[(this.retryOffset + attempted) % locations.length];
      const result = await this.cli.run(inspectArgs(probe(), [location], this.expansionPaths()), { timeoutMs: 5000 });
      const read = this.apply(result, values => {
        this.watchedValues.set(location, matchLocations([location], values).get(location) ?? []);
        fresh.push(...values);
      }, message => this.watchedValues.set(location, [errorValue(location, message)]));
      ok ||= read;
      if (!result.ok && result.failure.kind !== 'failed') {
        const message = result.failure.message;
        for (const pending of locations) {
          this.watchedValues.set(pending, [errorValue(pending, message)]);
        }
        // A transport/session failure invalidates earlier recovery samples too.
        fresh.splice(freshStart);
        this.watchedError = message;
        break;
      }
    }
    this.retryOffset = (this.retryOffset + Math.max(1, attempted)) % locations.length;
    return ok;
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
