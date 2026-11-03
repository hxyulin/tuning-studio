import * as assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import type { SessionController } from '../session';
import type { CliResult } from '../cli';

class Emitter<T> {
  private listeners: ((event: T) => void)[] = [];
  event = (listener: (event: T) => void) => { this.listeners.push(listener); return { dispose() {} }; };
  fire(event: T) { this.listeners.forEach(listener => listener(event)); }
  dispose() {}
}

function harness() {
  const processStart = new Emitter<{ execution: unknown; processId: number }>();
  const processEnd = new Emitter<{ execution: unknown; exitCode: number }>();
  const taskEnd = new Emitter<{ execution: unknown }>();
  const tasks: { task: { execution: { options: { env: Record<string, string> } } }; terminate(): void }[] = [];
  const calls: string[][] = [];
  const errors: string[] = [];
  const timers = new Map<number, () => void>();
  let nextTimer = 0;
  const sent: string[] = [];
  let terminations = 0;
  let selectedProbe = '1234:5678:original';
  let running = false;
  let stopResult: CliResult = { ok: true, stdout: 'stopping session', stderr: '' };
  const exports: Record<string, unknown> = {};
  class ProcessExecution {
    constructor(readonly command: string, readonly args: string[], readonly options: unknown) {}
  }
  class Task {
    constructor(readonly definition: unknown, readonly scope: unknown, readonly name: string, readonly source: string, readonly execution: unknown) {}
  }
  const folder = { uri: { fsPath: '/workspace' } };
  const build = { name: 'build', source: 'Workspace', definition: { type: 'shell' }, scope: folder };
  runInNewContext(readFileSync(path.join(__dirname, '../session.js'), 'utf8'), {
    exports, process, setTimeout: (fn: () => void, ms: number) => { const id = ++nextTimer; if (ms >= 1500) timers.set(id, fn); return id; }, clearTimeout: (id: number) => timers.delete(id),
    require: (name: string) => {
      if (name === './settings') return { probe: () => selectedProbe };
      if (name !== 'vscode') return require(name.startsWith('./') ? path.join(__dirname, '..', name) : name);
      return {
        EventEmitter: Emitter, ProcessExecution, Task,
        TaskRevealKind: { Always: 1 }, TaskPanelKind: { Dedicated: 1 }, TaskScope: { Workspace: 1 },
        commands: { executeCommand() {} },
        window: { terminals: [{ processId: Promise.resolve(123), sendText: (text: string) => sent.push(text) }], showInformationMessage() {}, showWarningMessage() {}, showErrorMessage: (message: string) => errors.push(message) },
        debug: { onDidStartDebugSession: () => ({ dispose() {} }), onDidTerminateDebugSession: () => ({ dispose() {} }) },
        tasks: {
          onDidStartTaskProcess: processStart.event, onDidEndTaskProcess: processEnd.event, onDidEndTask: taskEnd.event,
          fetchTasks: async () => [build],
          executeTask: async (task: unknown) => {
            const execution = { task, terminate() { terminations++; } };
            tasks.push(execution as typeof tasks[number]);
            return execution;
          },
        },
      };
    },
  });
  const Controller = exports.SessionController as new (...args: unknown[]) => SessionController;
  const controller = new Controller({ run: async (args: string[]) => {
    calls.push(args);
    if (args[0] === 'stop') return stopResult;
    return running ? { ok: true, stdout: 'running', stderr: '' } : { ok: false, stdout: '', stderr: '', failure: { kind: 'no-session', message: 'no session' } };
  } }, { appendLine() {} });
  const command = { kind: 'cargo' as const, args: ['run'], env: { RUNNER: 'tuning-studio run' }, triple: 'arm' };
  return {
    controller, command, folder, tasks, calls, errors, sent,
    start: () => processStart.fire({ execution: tasks.at(-1), processId: 123 }),
    expire: () => { const pending = [...timers.values()]; timers.clear(); pending.forEach(fn => fn()); },
    get terminations() { return terminations; },
    set running(value: boolean) { running = value; },
    set selectedProbe(value: string) { selectedProbe = value; },
    set stopResult(value: CliResult) { stopResult = value; },
    end: () => processEnd.fire({ execution: tasks.at(-1), exitCode: 0 }),
  };
}

test('Stop requests detach for its own token and original probe without terminating the terminal', async () => {
  const h = harness();
  await h.controller.run(h.command, h.folder as never);
  h.running = true;
  await h.controller.check();
  const token = h.tasks[0].task.execution.options.env.TUNING_STUDIO_OWNER_TOKEN;
  assert.match(token, /^[a-f0-9-]{36}$/);
  h.selectedProbe = 'other-probe';
  const stopping = Promise.all([h.controller.stop(), h.controller.stop()]);
  await new Promise<void>(resolve => setImmediate(resolve));
  h.end();
  await stopping;
  const stops = h.calls.filter(args => args[0] === 'stop');
  assert.equal(stops.length, 1);
  assert.deepEqual([...stops[0]], ['stop', '--probe', '1234:5678:original', '--owner', token]);
  assert.equal(h.terminations, 0);
  assert.equal(h.controller.ownsSession, false, 'Stop waits until the process exits');
  h.end();
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(h.controller.ownsSession, false);
  assert.equal(h.controller.state.kind, 'none');
  h.controller.dispose();
});

test('a rejected Stop leaves the session running and reports the reason', async () => {
  const h = harness();
  await h.controller.run(h.command, h.folder as never);
  h.running = true;
  await h.controller.check();
  h.stopResult = { ok: false, stdout: '', stderr: '', failure: { kind: 'failed', message: 'session owner does not match' } };
  await h.controller.stop();
  assert.equal(h.terminations, 0);
  assert.equal(h.controller.ownsSession, true);
  assert.match(h.errors[0], /owner does not match/);
  h.end();
  h.controller.dispose();
});

test('Stop cancels Cargo before a session exists, and refuses external sessions', async () => {
  const h = harness();
  await h.controller.stop();
  assert.equal(h.calls.length, 0);
  await h.controller.run(h.command, h.folder as never);
  h.stopResult = { ok: false, stdout: '', stderr: '', failure: { kind: 'no-session', message: 'no running session' } };
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(h.controller.state.kind, 'starting');
  assert.equal(h.controller.ownsSession, true);
  const stopping = h.controller.stop();
  await new Promise<void>(resolve => setImmediate(resolve));
  h.end();
  await stopping;
  assert.equal(h.terminations, 1, JSON.stringify({ calls: h.calls, errors: h.errors, state: h.controller.state }));
  h.end();
  h.controller.dispose();
});

test('stopping a build prevents the firmware run from starting', async () => {
  const h = harness();
  const run = h.controller.run({ kind: 'task', buildTask: 'build', executable: 'tuning-studio', args: ['run', 'fw.elf'] }, h.folder as never);
  // Let status, task lookup, and executeTask settle, without ending the build.
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.equal(h.controller.state.kind, 'building');
  const stopping = h.controller.stop();
  await new Promise<void>(resolve => setImmediate(resolve));
  h.end();
  await stopping;
  assert.equal(h.terminations, 1);
  assert.equal(h.calls.some(args => args[0] === 'stop'), false);
  h.end();
  await run;
  assert.equal(h.tasks.length, 1);
  assert.equal(h.controller.state.kind, 'none');
  h.controller.dispose();
});

test('Stop interrupts an owned terminal that stays alive after detach, then terminates it', async () => {
  const h = harness();
  await h.controller.run(h.command, h.folder as never);
  h.running = true; await h.controller.check(); h.start();
  const stopping = h.controller.stop();
  const settle = () => new Promise<void>(resolve => setImmediate(resolve));
  await settle(); h.expire(); await settle();
  assert.deepEqual(h.sent, ['\u0003']);
  assert.equal(h.terminations, 0);
  h.expire(); await settle();
  assert.equal(h.terminations, 1);
  h.end(); await stopping;
  assert.equal(h.controller.ownsSession, false);
  h.controller.dispose();
});
