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
