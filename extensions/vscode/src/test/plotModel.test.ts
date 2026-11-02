import * as assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import type { PlotModel } from '../plotView';

test('a frozen plot keeps its time window when the webview is recreated or a series is added', async () => {
  // Load the real model with only the editor services replaced. The webview
  // receives init again when its panel is reopened.
  const exports: Record<string, unknown> = {};
  runInNewContext(readFileSync(path.join(__dirname, '../plotView.js'), 'utf8'), {
    exports,
    require: (name: string) => {
      if (name === 'vscode') {
        return { EventEmitter: class { event = () => {}; fire() {} } };
      }
      if (name === './settings') {
        return { plotWindow: () => 30, plotCapacity: () => 100 };
      }
      return require(name.startsWith('./') ? path.join(__dirname, '..', name) : name);
    },
  });
  const Model = exports.PlotModel as new (state: unknown) => PlotModel;
  const model = new Model({ get: () => ['speed'], update: async () => {} });
  model.record({ time: 1, numbers: new Map([['speed', 100]]), ok: true });
  model.frozen = true;
  model.record({ time: 2, numbers: new Map([['speed', 180]]), ok: true });
  const reopened = model.init();
  assert.equal(reopened.type, 'init');
  if (reopened.type === 'init') {
    assert.equal(reopened.frozen, true);
    assert.equal(reopened.frozenEnd, 1);
    assert.equal(reopened.series[0].t.at(-1), 2, 'recording continues while the visible window is frozen');
  }
  await model.set('setpoint', true);
  assert.equal(model.frozenEnd, 1, 'changing series must not move the frozen window');
  model.frozen = false;
  model.frozen = true;
  assert.equal(model.frozenEnd, 2, 'freezing again captures the latest sample');
});

test('a single plot keeps hidden samples and persists visibility and title', async () => {
  const exports: Record<string, unknown> = {};
  runInNewContext(readFileSync(path.join(__dirname, '../plotView.js'), 'utf8'), {
    exports,
    require: (name: string) => {
      if (name === 'vscode') return { EventEmitter: class { event = () => {}; fire() {} } };
      if (name === './settings') return { plotWindow: () => 30, plotCapacity: () => 100 };
      return require(name.startsWith('./') ? path.join(__dirname, '..', name) : name);
    },
  });
  const Model = exports.PlotModel as new (state: unknown) => PlotModel;
  const saved = new Map<string, unknown>();
  const state = { get: (key: string, fallback: unknown) => saved.get(key) ?? fallback, update: async (key: string, value: unknown) => { saved.set(key, structuredClone(value)); } };
  const model = new Model(state);
  model.choices = ['speed', 'target'];
  await model.layout('seriesAdd', '', 'speed', '');
  const first = model.plots[0].id;
  await model.layout('seriesAdd', first, 'target', '');
  await model.layout('add', '', '', '');
  assert.equal(model.plots.length, 1, 'additional plots are not supported');
  model.record({ time: 1, numbers: new Map([['speed', 12], ['target', 15]]), ok: true });
  await model.layout('seriesToggle', first, 'speed', '');
  model.record({ time: 2, numbers: new Map([['speed', 14]]), ok: true });
  const init = model.init();
  if (init.type === 'init') {
    assert.equal(init.series.find(s => s.path === 'speed')?.v.at(-1), 14, 'hidden values still collect samples');
    assert.equal(init.plots[0].hidden?.[0], 'speed');
  }
  await model.layout('rename', first, '', 'Motor response');
  const restored = new Model(state);
  assert.equal(restored.plots.length, 1);
  assert.equal(restored.plots[0].title, 'Motor response');
  assert.deepEqual([...restored.plots[0].hidden!], ['speed']);
  await model.layout('seriesToggle', first, 'speed', '');
  assert.equal(model.plots[0].hidden?.length, 0);
  await model.layout('seriesRemove', first, 'speed', '');
  assert.equal(model.has('speed'), false, 'removing a variable releases its buffer');
  await model.layout('seriesAdd', first, 'unknown', '');
  assert.equal(model.has('unknown'), false, 'webview cannot subscribe an unknown path');
  await model.layout('remove', first, '', '');
  assert.equal(model.plots.length, 0);
  assert.equal(model.paths.length, 0);
  await model.layout('seriesAdd', '', 'target', '');
  assert.equal(model.plots.length, 1, 'adding a variable recreates the closed plot');
  const legacy = new Map<string, unknown>([
    ['tuningStudio.plotted', ['speed', 'target']],
    ['tuningStudio.layout', { columns: 2, plots: [
      { id: 'old-1', title: 'Motor', paths: ['speed'] },
      { id: 'old-2', title: 'Target', paths: ['target', 'speed'] },
    ] }],
  ]);
  const migrated = new Model({ get: (key: string) => legacy.get(key), update: async () => {} });
  assert.equal(migrated.plots.length, 1);
  assert.equal(migrated.plots[0].title, 'Motor');
  assert.deepEqual([...migrated.plots[0].paths], ['speed', 'target']);

});
