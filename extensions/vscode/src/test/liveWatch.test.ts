import * as assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import { flattenValues, parseInspect } from '../inspect';
import type { LiveWatch, PollSample } from '../liveWatch';
import type { PlotModel } from '../plotView';

class Emitter<T> {
  private readonly listeners: ((value: T) => void)[] = [];
  event = (listener: (value: T) => void) => { this.listeners.push(listener); return { dispose() {} }; };
  fire(value: T) { this.listeners.forEach(listener => listener(value)); }
  dispose() {}
}

function load(module: string): Record<string, unknown> {
  const exports: Record<string, unknown> = {};
  runInNewContext(readFileSync(path.join(__dirname, '..', module + '.js'), 'utf8'), {
    exports, performance: { now: () => 1000 },
    setTimeout: () => 1, clearTimeout() {},
    require: (name: string) => {
      if (name === 'vscode') return { EventEmitter: Emitter, commands: { executeCommand() {} } };
      if (name === './settings') return { pollInterval: () => 200, probe: () => '', plotWindow: () => 30, plotCapacity: () => 100 };
      return require(name.startsWith('./') ? path.join(__dirname, '..', name) : name);
    },
  });
  return exports;
}

test('whole-call watch failures recover individual values without plotting failed or stale rows', async () => {
  let timeout = false;
  const individual: string[] = [];
  const cli = { run: async (args: string[]) => {
    const locations = args.includes('--') ? args.slice(args.indexOf('--') + 1) : [];
    if (!locations.length) return { ok: true, stdout: '{"version":1,"values":[]}', stderr: '' };
    if (locations.length === 1) individual.push(locations[0]);
    if (timeout || locations.length > 1 || locations[0] === 'bad') return {
      ok: false, stdout: '', stderr: '', failure: { kind: timeout ? 'timeout' : 'failed', message: timeout ? 'timeout' : 'unknown symbol bad' },
    };
    return { ok: true, stderr: '', stdout: JSON.stringify({ version: 1, values: [{
      path: locations[0], type: 'u32', kind: 'raw', writable: true, display: '12', number: 12,
    }] }) };
  } };
  const Watch = load('liveWatch').LiveWatch as new (...args: unknown[]) => LiveWatch;
  const paths = ['bad', ...Array.from({ length: 9 }, (_, i) => `good${i}`)];
  const state = { get: (key: string, fallback: unknown) => key === 'tuningStudio.watches' ? paths : fallback, update: async () => {} };
  const watch = new Watch(cli, { isReady: true, onDidChange: () => ({ dispose() {} }), markFailure() {}, markLost() {} }, state);
  const samples: PollSample[] = [];
  watch.onDidSample(sample => samples.push(sample));
  const poll = () => (watch as unknown as { poll(): Promise<void> }).poll();
  await poll();
  assert.equal(watch.lookup('bad')?.error, 'unknown symbol bad');
  assert.equal(watch.lookup('bad')?.writable, false);
  assert.equal(watch.lookup('good0')?.number, 12);
  assert.equal(watch.watchedError, undefined);
  assert.equal(samples[0].numbers.get('good0'), 12);
  assert.equal(samples[0].numbers.has('bad'), false);
  assert.equal(individual.length, 8);
  await poll();
  assert.equal(individual.length, 16);
  assert.equal(new Set(individual).size, 10, 'recovery rotates through long lists');
  timeout = true;
  await poll();
  assert.equal(individual.length, 16, 'transport failures are not retried');
  assert.equal(watch.watchedValues.size, 0, 'stale writable rows are cleared');
  assert.equal(watch.watchedError, 'timeout');
  assert.equal(samples[2].numbers.size, 0);
  watch.dispose();
});

test('numeric struct fields reach plot buffers without expanding their watch rows', async () => {
  const stdout = JSON.stringify({ version: 1, values: [{
    path: 'motor.state', type: 'MotorSnapshot', kind: 'watch', writable: false,
    display: 'motor.state: MotorSnapshot', children: [{
      path: 'motor.state.speed', label: 'speed', type: 'f32', kind: 'watch', writable: false,
      display: 'motor.state.speed: f32 = 12.5', number: 12.5,
    }],
  }] });
  const Watch = load('liveWatch').LiveWatch as new (...args: unknown[]) => LiveWatch;
  const state = { get: (_: string, fallback: unknown) => fallback, update: async () => {} };
  const watch = new Watch(
    { run: async () => ({ ok: true, stdout, stderr: '' }) },
    { isReady: true, onDidChange: () => ({ dispose() {} }) }, state,
  );
  const samples: PollSample[] = [];
  watch.onDidSample(sample => samples.push(sample));
  await (watch as unknown as { poll(): Promise<void> }).poll();
  assert.equal(watch.lookup('motor.state.speed')?.number, 12.5);
  assert.equal(watch.lookup('motor.state.speed')?.writable, false);
  assert.equal(samples[0].numbers.get('motor.state.speed'), 12.5);

  const Model = load('plotView').PlotModel as new (...args: unknown[]) => PlotModel;
  const plot = new Model(state);
  plot.choices = flattenValues(parseInspect(stdout).values).filter(v => v.number !== undefined).map(v => v.path);
  await plot.layout('seriesAdd', '', 'motor.state.speed', '');
  plot.record(samples[0]);
  const init = plot.init();
  assert.equal(init.type, 'init');
  if (init.type === 'init') assert.equal(init.series[0].v[0], 12.5);
  watch.dispose();
});


test('pointer polls stop after collapse unless a plotted field needs them', async () => {
  const calls: string[][] = [];
  const cli = { run: async (args: string[]) => {
    calls.push(args);
    const expanded = args.includes('--expand');
    return { ok: true, stderr: '', stdout: JSON.stringify({ version: 1, values: [{
      path: 'BOXED', type: 'Box<Controller>', kind: 'raw', writable: false, display: 'BOXED: Box<Controller> = 0x20000000', expandable: true,
      ...(expanded ? { children: [{ path: 'BOXED.*', label: '*', type: 'Controller', kind: 'raw', writable: false, display: 'BOXED.*: Controller', children: [
        { path: 'BOXED.*.kp', label: 'kp', type: 'f32', kind: 'raw', writable: true, display: 'BOXED.*.kp: f32 = 0.5', number: 0.5 },
      ] }] } : {}),
    }] }) };
  } };
  const saved = new Map<string, unknown>();
  const state = { get: (key: string, fallback: unknown) => saved.get(key) ?? fallback, update: async (key: string, value: unknown) => { saved.set(key, structuredClone(value)); } };
  const Watch = load('liveWatch').LiveWatch as new (...args: unknown[]) => LiveWatch;
  const session = { isReady: true, onDidChange: () => ({ dispose() {} }) };
  const watch = new Watch(cli, session, state);
  const poll = () => (watch as unknown as { poll(): Promise<void> }).poll();
  await poll();
  assert.equal(calls.at(-1)?.includes('--expand'), false);
  await watch.setExpanded('BOXED', true);
  await poll();
  assert.equal(watch.lookup('BOXED.*.kp')?.number, 0.5);
  watch.setPlotted(['BOXED.*.kp']);
  await watch.setExpanded('BOXED', false);
  await poll();
  assert.equal(calls.at(-1)?.includes('--expand'), true, 'plots retain their pointer dependencies');
  watch.setPlotted([]);
  await poll();
  assert.equal(calls.at(-1)?.includes('--expand'), false);
  await watch.setExpanded('BOXED', true);
  watch.dispose();
  const reopened = new Watch(cli, session, state);
  await (reopened as unknown as { poll(): Promise<void> }).poll();
  assert.equal(calls.at(-1)?.includes('--expand'), true, 'open pointer requests persist');
  reopened.dispose();
});


test('pointer expansion limits return a reason instead of leaving a pending empty row', async () => {
  const Watch = load('liveWatch').LiveWatch as new (...args: unknown[]) => LiveWatch;
  const state = { get: (_: string, fallback: unknown) => fallback, update: async () => {} };
  const watch = new Watch({}, { isReady: false, onDidChange: () => ({ dispose() {} }) }, state);
  for (let i = 0; i < 64; i++) assert.equal(await watch.setExpanded(`PTR${i}`, true), true);
  assert.equal(await watch.setExpanded('EXTRA', true), false);
  await watch.setExpanded('PTR0', false);
  assert.equal(await watch.setExpanded('EXTRA', true), true);
  watch.dispose();
});
