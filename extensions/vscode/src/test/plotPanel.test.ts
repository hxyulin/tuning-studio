import * as assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

class Element {
  children: Element[] = [];
  style: Record<string, string> = {};
  className = '';
  textContent = '';
  value = '';
  clientWidth = 800;
  clientHeight = 400;
  listeners = new Map<string, ((e: any) => void)[]>();
  get options() { return this.children; }
  get offsetLeft() { return parseFloat(this.style.left ?? '12'); }
  get offsetTop() { return parseFloat(this.style.top ?? '8'); }
  get offsetWidth() { return parseFloat(this.style.width ?? '240'); }
  get offsetHeight() { return parseFloat(this.style.height ?? '180'); }
  classList = { add() {}, remove() {}, toggle() {} };
  append(...elements: Element[]) { this.children.push(...elements); }
  replaceChildren() { this.children = []; }
  setAttribute() {}
  getBoundingClientRect() { return { right: 800 }; }
  setPointerCapture() {}
  addEventListener(type: string, fn: (e: any) => void) { this.listeners.set(type, [...this.listeners.get(type) ?? [], fn]); }
  removeEventListener(type: string, fn: (e: any) => void) { this.listeners.set(type, (this.listeners.get(type) ?? []).filter(f => f !== fn)); }
  fire(type: string, data: Record<string, unknown> = {}) { this.listeners.get(type)?.slice().forEach(fn => fn({ button: 0, preventDefault() {}, ...data })); }
  contains(node: Element): boolean { return this === node || this.children.some(c => c.contains(node)); }
}

test('plot legend moves, restores its bounds, and accepts only available watch drops', () => {
  const lanes = new Element();
  const body = new Element();
  const posted: Record<string, unknown>[] = [];
  let saved: any;
  let receive: (e: unknown) => void = () => {};
  const frames: (() => void)[] = [];
  const observer = class { observe() {} disconnect() {} };
  runInNewContext(readFileSync(path.join(__dirname, '../../media/plot.js'), 'utf8'), {
    document: { body, getElementById: (id: string) => id === 'lanes' ? lanes : new Element(), createElement: () => new Element() },
    window: { addEventListener: (_type: string, fn: (e: unknown) => void) => { receive = fn; } },
    ResizeObserver: observer, MutationObserver: observer,
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    requestAnimationFrame: (fn: () => void) => { frames.push(fn); return frames.length; },
    acquireVsCodeApi: () => ({ postMessage: (m: Record<string, unknown>) => posted.push(m), getState: () => saved, setState: (s: unknown) => { saved = s; } }),
  });
  const init = { type: 'init', plots: [], choices: ['motor.speed'], series: [], capacity: 100, windowSeconds: 30, frozen: false, frozenEnd: 0 };
  receive({ data: init });
  let plotBody = lanes.children[0].children[1];
  let legend = plotBody.children[0];
  const grip = legend.children[0];
  grip.fire('pointerdown', { clientX: 20, clientY: 20, pointerId: 1 });
  grip.fire('pointermove', { clientX: 100, clientY: 70 });
  grip.fire('pointerup');
  assert.equal(saved.legendBox.x, 92);
  assert.equal(saved.legendBox.y, 58);
  legend.style.width = '310px'; legend.style.height = '210px'; legend.fire('pointerup');
  assert.equal(saved.legendBox.width, 310);
  assert.equal(saved.legendBox.height, 210);
  receive({ data: init });
  plotBody = lanes.children[0].children[1]; legend = plotBody.children[0];
  // The first callback after init places the legend; the second draws canvas.
  frames[2]();
  assert.equal(legend.style.left, '92px');
  assert.equal(legend.style.width, '310px');
  const drop = (value: string) => plotBody.fire('drop', { dataTransfer: { getData: () => value } });
  drop('motor.speed'); drop('unknown');
  assert.equal(posted.filter(m => m.action === 'seriesAdd').length, 1);
  assert.equal(posted.at(-1)?.path, 'motor.speed');
  const chart = plotBody.children[1];
  const [canvas, axis] = chart.children;
  axis.fire('pointerdown', { clientX: 100, pointerId: 1 });
  axis.fire('pointermove', { clientX: 220 }); axis.fire('pointerup');
  assert.ok(saved.zoom.x < 1, 'dragging time right stretches horizontally');
  assert.equal(saved.zoom.y, 1, 'time drag leaves the vertical scale alone');
  canvas.fire('pointerdown', { clientX: 780, clientY: 200, pointerId: 1 });
  canvas.fire('pointermove', { clientY: 80 }); canvas.fire('pointerup');
  assert.ok(saved.zoom.y > 1, 'dragging values up stretches vertically');
  const x = saved.zoom.x;
  chart.fire('wheel', { target: { closest: () => null }, shiftKey: true, deltaY: -100, deltaMode: 0 });
  assert.equal(saved.zoom.x, x, 'Shift+scroll changes only the vertical scale');
  assert.ok(saved.zoom.y > Math.E);
  axis.fire('dblclick'); assert.equal(saved.zoom.x, 1); assert.ok(saved.zoom.y > 1);
  canvas.fire('dblclick'); assert.equal(saved.zoom.x, 1); assert.equal(saved.zoom.y, 1);
  axis.fire('keydown', { key: 'ArrowRight' }); assert.equal(saved.zoom.x, 0.8);
  axis.fire('keydown', { key: 'Home' }); assert.equal(saved.zoom.x, 1);
});
