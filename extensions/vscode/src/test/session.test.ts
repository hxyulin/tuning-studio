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
  const calls: string[][] = [];
  const timers = new Map<number, () => void>();
  let nextTimer = 0;
  let result: CliResult = { ok: false, stdout: '', stderr: '', failure: { kind: 'no-session', message: 'no session' } };
  let pending: Promise<CliResult> | undefined;
  const exports: Record<string, unknown> = {};
  runInNewContext(readFileSync(path.join(__dirname, '../session.js'), 'utf8'), {
    exports,
    setTimeout: (fn: () => void) => { const id = ++nextTimer; timers.set(id, fn); return id; },
    clearTimeout: (id: number) => timers.delete(id),
    require: (name: string) => {
      if (name === './settings') return { probe: () => '1234:5678:board' };
      // No task, terminal, debug-lifecycle, or process API is available to the observer.
      if (name === 'vscode') return { EventEmitter: Emitter };
      return require(name.startsWith('./') ? path.join(__dirname, '..', name) : name);
    },
  });
  const Controller = exports.SessionController as new (...args: unknown[]) => SessionController;
  const controller = new Controller({ run: async (args: string[]) => {
    calls.push(args);
    return pending ?? result;
  } });
  return {
    controller, calls, timers,
    set result(value: CliResult) { result = value; },
    set pending(value: Promise<CliResult>) { pending = value; },
  };
}

test('session observation only issues status and tracks availability', async () => {
  const h = harness();
  assert.equal((await h.controller.check()).kind, 'none');
  h.result = { ok: true, stdout: 'running\nmore details', stderr: '' };
  const state = await h.controller.check();
  assert.equal(state.kind, 'ready');
  if (state.kind === 'ready') assert.equal(state.status, 'running');
  assert.equal(h.controller.isReady, true);
  h.controller.markLost();
  assert.equal(h.controller.state.kind, 'none');
  assert.equal(h.calls.length, 2);
  for (const call of h.calls) assert.deepEqual([...call], ['status', '--probe', '1234:5678:board']);
  h.controller.dispose();
});

test('concurrent checks share a request and disposal cancels monitoring', async () => {
  const h = harness();
  let resolve!: (value: CliResult) => void;
  h.pending = new Promise(done => { resolve = done; });
  h.controller.setMonitoring(true);
  assert.equal(h.timers.size, 1);
  const first = h.controller.check();
  const second = h.controller.check();
  assert.equal(h.calls.length, 1);
  h.controller.dispose();
  assert.equal(h.timers.size, 0);
  resolve({ ok: true, stdout: 'running', stderr: '' });
  await Promise.all([first, second]);
  assert.equal(h.timers.size, 0, 'an in-flight response must not restart a disposed monitor');
});

test('session failures remain actionable and hiding views stops status polling', async () => {
  const h = harness();
  h.controller.setMonitoring(true);
  h.result = { ok: false, stdout: '', stderr: '', failure: { kind: 'several-sessions', message: 'select a probe', probes: ['a', 'b'] } };
  const state = await h.controller.check();
  assert.equal(state.kind, 'error');
  if (state.kind === 'error') assert.equal(state.failure.message, 'select a probe');
  h.controller.setMonitoring(false);
  assert.equal(h.timers.size, 0);
  h.controller.dispose();
});

test('extension manifest contributes tuning without run controls or task definitions', () => {
  const manifest = JSON.parse(readFileSync(path.join(__dirname, '../../package.json'), 'utf8'));
  const contributes = manifest.contributes;
  assert.equal(contributes.taskDefinitions, undefined);
  for (const id of ['tuningStudio.run', 'tuningStudio.stop']) {
    assert.equal(contributes.commands.some((command: { command: string }) => command.command === id), false);
    for (const entries of Object.values(contributes.menus) as { command: string }[][]) {
      assert.equal(entries.some(entry => entry.command === id), false);
    }
  }
  for (const name of ['runMode', 'buildTask', 'chip', 'cargoTarget', 'cargoArgs']) {
    assert.equal(contributes.configuration.properties[`tuningStudio.${name}`], undefined);
  }
});
