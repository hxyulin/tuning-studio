import * as vscode from 'vscode';
import { Cli } from './cli';
import { Failure, statusArgs } from './cliText';
import { RunCommand } from './runPlan';
import { probe } from './settings';

export type SessionState =
  | { kind: 'unknown' }
  | { kind: 'none' }
  | { kind: 'building'; task: string }
  | { kind: 'starting'; detail: string }
  | { kind: 'ready'; owned: boolean; status: string }
  | { kind: 'error'; failure: Failure };

const TASK_TYPE = 'tuning-studio';
const STARTING_CHECK_MS = 500;
const IDLE_CHECK_MS = 2000;

/**
 * Tracks whether a tuning-studio session serves this probe, and owns the
 * session that Run starts. It never stops a session it did not start.
 */
export class SessionController implements vscode.Disposable {
  private stateValue: SessionState = { kind: 'unknown' };
  private owned?: vscode.TaskExecution;
  private ownedRole?: 'build' | 'run';
  private stopRequested = false;
  private checkTimer?: NodeJS.Timeout;
  private inflight?: Promise<SessionState>;
  private monitorWanted = false;
  private readonly probeRsSessions = new Set<string>();
  private readonly emitter = new vscode.EventEmitter<SessionState>();
  private readonly disposables: vscode.Disposable[] = [];

  readonly onDidChange = this.emitter.event;

  constructor(
    private readonly cli: Cli,
    private readonly log: vscode.OutputChannel,
  ) {
    const active = vscode.debug.activeDebugSession;
    if (active?.type === 'probe-rs-debug') {
      this.probeRsSessions.add(active.id);
    }
    this.disposables.push(
      this.emitter,
      vscode.debug.onDidStartDebugSession((s) => {
        if (s.type !== 'probe-rs-debug') {
          return;
        }
        this.probeRsSessions.add(s.id);
        if (this.stateValue.kind === 'ready' || this.stateValue.kind === 'starting') {
          void vscode.window.showWarningMessage(
            'A probe-rs debug session started while tuning-studio holds the probe. Stop one of them.',
          );
        }
      }),
      vscode.debug.onDidTerminateDebugSession((s) => this.probeRsSessions.delete(s.id)),
      vscode.tasks.onDidEndTaskProcess((e) => this.onTaskEnded(e.execution, e.exitCode)),
      // A task made only of dependsOn has no process; its end has no exit code.
      vscode.tasks.onDidEndTask((e) => this.onTaskEnded(e.execution, undefined)),
    );
  }

  get state(): SessionState {
    return this.stateValue;
  }

  get isReady(): boolean {
    return this.stateValue.kind === 'ready';
  }

  get ownsSession(): boolean {
    return this.owned !== undefined;
  }

  private setState(state: SessionState): void {
    this.stateValue = state;
    void vscode.commands.executeCommand('setContext', 'tuningStudio.owned', this.owned !== undefined);
    void vscode.commands.executeCommand(
      'setContext',
      'tuningStudio.busy',
      state.kind === 'building' || state.kind === 'starting',
    );
    this.emitter.fire(state);
    this.scheduleCheck();
  }

  /** Check for a session now and keep checking while a view is visible or Run is starting. */
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
    if (kind === 'building') {
      return;
    }
    if (!this.monitorWanted && kind !== 'starting' && kind !== 'unknown') {
      return;
    }
    const delay = kind === 'starting' ? STARTING_CHECK_MS : kind === 'unknown' ? 0 : IDLE_CHECK_MS;
    this.checkTimer = setTimeout(() => void this.check(), delay);
  }

  /** Run `status` once and update the state from its answer. Concurrent calls share one run. */
  check(): Promise<SessionState> {
    this.inflight ??= this.checkOnce().finally(() => (this.inflight = undefined));
    return this.inflight;
  }

  private async checkOnce(): Promise<SessionState> {
    const result = await this.cli.run(statusArgs(probe()), { timeoutMs: 5000 });
    const kind = this.stateValue.kind;
    if (kind === 'building') {
      return this.stateValue;
    }
    if (result.ok) {
      const status = result.stdout.split(/\r?\n/)[0]?.trim() || 'running';
      if (kind === 'starting') {
        this.log.appendLine('Session is ready.');
      }
      this.setState({ kind: 'ready', owned: this.ownedRole === 'run', status });
    } else if (result.failure.kind === 'no-session') {
      // A session Run started is not up until `status` reaches it.
      this.setState(kind === 'starting' ? this.stateValue : { kind: 'none' });
    } else if (kind === 'starting' && result.failure.kind === 'timeout') {
      this.setState(this.stateValue);
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

  /** Build if needed, start `tuning-studio run` as a task, and wait for its session. */
  async run(command: RunCommand, folder: vscode.WorkspaceFolder): Promise<void> {
    if (this.owned) {
      void vscode.window.showInformationMessage('Run is already in progress.');
      return;
    }
    if (this.probeRsSessions.size > 0) {
      void vscode.window.showErrorMessage(
        'A probe-rs debug session holds the probe. Stop it before Run, since only one program can own the probe.',
      );
      return;
    }
    const before = await this.check();
    if (before.kind === 'ready') {
      void vscode.window.showInformationMessage(
        'A tuning-studio session is already running for this probe; Tuning Studio uses it. Stop it in its terminal to run again.',
      );
      return;
    }
    if (before.kind === 'error' && before.failure.kind !== 'failed') {
      void vscode.window.showErrorMessage(`Cannot run: ${before.failure.message}`);
      return;
    }
    this.stopRequested = false;

    if (command.kind === 'task' && command.buildTask) {
      const build = await findTask(command.buildTask, folder);
      if (!build) {
        this.setState({
          kind: 'error',
          failure: { kind: 'failed', message: `No task labelled "${command.buildTask}"` },
        });
        return;
      }
      this.log.appendLine(`Running build task "${command.buildTask}".`);
      this.setState({ kind: 'building', task: command.buildTask });
      const exitCode = await this.runOwned(build, 'build');
      if (this.stopRequested) {
        this.setState({ kind: 'none' });
        return;
      }
      if (exitCode !== 0) {
        this.setState({
          kind: 'error',
          failure: { kind: 'failed', message: exitCode === undefined
            ? `Build task "${command.buildTask}" did not report success; use a process or shell build task`
            : `Build task "${command.buildTask}" exited with ${exitCode}` },
        });
        return;
      }
    }

    const task = runTask(command, folder);
    this.log.appendLine(
      command.kind === 'cargo'
        ? `Starting cargo ${command.args.join(' ')} with ${Object.entries(command.env)
            .map(([k, v]) => `${k}="${v}"`)
            .join(' ')}`
        : `Starting ${command.executable} ${command.args.join(' ')}`,
    );
    this.setState({
      kind: 'starting',
      detail: command.kind === 'cargo' ? 'cargo run' : 'tuning-studio run',
    });
    void this.runOwned(task, 'run').then((exitCode) => {
      const wasReady = this.stateValue.kind === 'ready';
      if (this.stopRequested || exitCode === 0 || exitCode === undefined) {
        this.log.appendLine(`Run ended${exitCode === undefined ? '' : ` with ${exitCode}`}.`);
        this.setState({ kind: 'none' });
      } else {
        this.setState({
          kind: 'error',
          failure: {
            kind: 'failed',
            message: `${wasReady ? 'The session' : 'Run'} exited with ${exitCode}; see its terminal`,
          },
        });
      }
    }).catch((error: unknown) => {
      this.setState({ kind: 'error', failure: {
        kind: 'failed', message: `Could not start Run: ${error instanceof Error ? error.message : String(error)}`,
      } });
    });
  }

  /** Stop the build or session Run started. Sessions started elsewhere are left alone. */
  stop(): void {
    if (!this.owned) {
      void vscode.window.showInformationMessage(
        'Tuning Studio did not start this session. Stop it in the terminal that runs it.',
      );
      return;
    }
    this.stopRequested = true;
    this.log.appendLine('Stopping the session Run started.');
    this.owned.terminate();
  }

  private pendingEnd?: { execution: vscode.TaskExecution; resolve: (code: number | undefined) => void };

  private async runOwned(task: vscode.Task, role: 'build' | 'run'): Promise<number | undefined> {
    const execution = await vscode.tasks.executeTask(task);
    this.owned = execution;
    this.ownedRole = role;
    void vscode.commands.executeCommand('setContext', 'tuningStudio.owned', true);
    const code = await new Promise<number | undefined>((resolve) => {
      this.pendingEnd = { execution, resolve };
    });
    this.owned = undefined;
    this.ownedRole = undefined;
    return code;
  }

  private onTaskEnded(execution: vscode.TaskExecution, exitCode: number | undefined): void {
    const pending = this.pendingEnd;
    if (pending && sameExecution(pending.execution, execution)) {
      this.pendingEnd = undefined;
      pending.resolve(exitCode);
    }
  }

  dispose(): void {
    if (this.checkTimer) {
      clearTimeout(this.checkTimer);
    }
    this.disposables.forEach((d) => d.dispose());
  }
}

function sameExecution(a: vscode.TaskExecution, b: vscode.TaskExecution): boolean {
  if (a === b) {
    return true;
  }
  const x = a.task;
  const y = b.task;
  return x.name === y.name && x.source === y.source && x.definition.type === y.definition.type;
}

/** Find a task by the label tasks.json gives it, or by `source: name`. */
async function findTask(label: string, folder: vscode.WorkspaceFolder): Promise<vscode.Task | undefined> {
  const tasks = await vscode.tasks.fetchTasks();
  const inFolder = (t: vscode.Task) =>
    t.scope === folder || t.scope === vscode.TaskScope.Workspace || t.scope === undefined ||
    (typeof t.scope === 'object' && t.scope.uri.toString() === folder.uri.toString());
  return (
    tasks.find((t) => t.name === label && inFolder(t)) ??
    tasks.find((t) => `${t.source}: ${t.name}` === label && inFolder(t)) ??
    tasks.find((t) => t.name === label)
  );
}

function runTask(command: RunCommand, folder: vscode.WorkspaceFolder): vscode.Task {
  const cwd = folder.uri.fsPath;
  const execution =
    command.kind === 'cargo'
      ? new vscode.ProcessExecution('cargo', command.args, { cwd, env: command.env })
      : new vscode.ProcessExecution(command.executable, command.args, { cwd });
  const task = new vscode.Task(
    { type: TASK_TYPE, action: 'run' },
    folder,
    'run',
    'tuning-studio',
    execution,
    command.kind === 'cargo' ? ['$rustc'] : [],
  );
  task.presentationOptions = {
    reveal: vscode.TaskRevealKind.Always,
    panel: vscode.TaskPanelKind.Dedicated,
    clear: true,
    focus: false,
  };
  task.isBackground = false;
  return task;
}
