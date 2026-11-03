import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

// A minimal DOM: enough of the element, focus and event model for media/watch.js.
type Listener = (event: FakeEvent) => void;
class FakeEvent {
  defaultPrevented = false;
  stopped = false;
  target!: FakeElement;
  constructor(public type: string, init: Record<string, unknown> = {}) { Object.assign(this, init); }
  preventDefault() { this.defaultPrevented = true; }
  stopPropagation() { this.stopped = true; }
}
class FakeElement {
  children: FakeElement[] = [];
  parent?: FakeElement;
  attributes = new Map<string, string>();
  classes = new Set<string>();
  listeners = new Map<string, Listener[]>();
  style: Record<string, string> = {};
  text = '';
  id = ''; title = ''; value = ''; type = ''; hidden = false; readOnly = false; spellcheck = true; tabIndex = 0;
  constructor(public doc: FakeDocument, public tagName: string) {}
  get className() { return [...this.classes].join(' '); }
  set className(value: string) { this.classes = new Set(value.split(/\s+/).filter(Boolean)); }
  classList = {
    add: (name: string) => { this.classes.add(name); },
    remove: (name: string) => { this.classes.delete(name); },
    contains: (name: string) => this.classes.has(name),
    toggle: (name: string, force?: boolean) => { const on = force ?? !this.classes.has(name); if (on) this.classes.add(name); else this.classes.delete(name); return on; },
  };
  get textContent(): string { return this.text + this.children.map(c => c.textContent).join(''); }
  set textContent(value: string) { this.text = value; this.children.forEach(c => { c.parent = undefined; }); this.children = []; }
  append(...nodes: FakeElement[]) { for (const node of nodes) { node.remove(); node.parent = this; this.children.push(node); } }
  replaceChildren(...nodes: FakeElement[]) { this.children.forEach(c => { c.parent = undefined; }); this.children = []; this.append(...nodes); }
  insertBefore(node: FakeElement, ref: FakeElement) { node.remove(); node.parent = this; this.children.splice(this.children.indexOf(ref), 0, node); }
  remove() { if (this.parent) { this.parent.children = this.parent.children.filter(c => c !== this); this.parent = undefined; } }
  contains(node: FakeElement | undefined): boolean { for (let n = node; n; n = n.parent) if (n === this) return true; return false; }
  setAttribute(name: string, value: string) { this.attributes.set(name, String(value)); }
  getAttribute(name: string) { return this.attributes.get(name) ?? null; }
  removeAttribute(name: string) { this.attributes.delete(name); }
  addEventListener(type: string, listener: Listener) { this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]); }
  dispatch(type: string, init: Record<string, unknown> = {}, bubbles = true): FakeEvent {
    const event = new FakeEvent(type, init);
    event.target = this;
    for (let n: FakeElement | undefined = this; n && !event.stopped; n = bubbles ? n.parent : undefined) n.listeners.get(type)?.forEach(l => l(event));
    if (bubbles && !event.stopped) this.doc.listeners.get(type)?.forEach(l => l(event));
    return event;
  }
  click() { this.dispatch('click'); }
  key(key: string, init: Record<string, unknown> = {}) { return this.dispatch('keydown', { key, ...init }); }
  focus() { this.doc.focusOn(this); }
  blur() { if (this.doc.activeElement === this) this.doc.focusOn(this.doc.body); }
  select() {}
  scrollIntoView() {}
  getBoundingClientRect() { return { left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0 }; }
  find(predicate: (e: FakeElement) => boolean): FakeElement[] {
    return this.children.flatMap(c => [...(predicate(c) ? [c] : []), ...c.find(predicate)]);
  }
  byClass(name: string) { return this.find(e => e.classes.has(name)); }
}
class FakeDocument {
  listeners = new Map<string, Listener[]>();
  body = new FakeElement(this, 'body');
  activeElement: FakeElement = this.body;
  createElement(tag: string) { return new FakeElement(this, tag); }
  getElementById(id: string) { return this.body.find(e => e.id === id)[0] ?? null; }
  addEventListener(type: string, listener: Listener) { this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]); }
  hasFocus() { return true; }
  focusOn(element: FakeElement) {
    const previous = this.activeElement;
    if (previous === element) return;
    this.activeElement = element;
    previous.dispatch('blur', {}, false);
    element.dispatch('focus', {}, false);
  }
}

function loadWebview(initialState: Record<string, unknown> = {}) {
  let savedState = initialState;
  const doc = new FakeDocument();
  for (const [tag, id] of [['p', 'status'], ['div', 'values'], ['p', 'empty'], ['form', 'add'], ['div', 'announce']]) {
    const element = doc.createElement(tag); element.id = id; doc.body.append(element);
  }
  const location = doc.createElement('input'); location.id = 'location'; doc.getElementById('add')!.append(location);
  const posted: Record<string, unknown>[] = [];
  const timers = new Map<number, () => void>();
  let timerId = 0;
  const windowListeners: Listener[] = [];
  const window = {
    innerWidth: 300, innerHeight: 600,
    addEventListener: (type: string, listener: Listener) => { if (type === 'message') windowListeners.push(listener); },
  };
  runInNewContext(readFileSync(path.join(__dirname, '../../media/watch.js'), 'utf8'), {
    document: doc, window, setTimeout: (fn: () => void) => { timers.set(++timerId, fn); return timerId; }, clearTimeout: (id: number) => timers.delete(id), acquireVsCodeApi: () => ({ postMessage: (m: Record<string, unknown>) => posted.push(structuredClone(m)), getState: () => savedState, setState: (value: Record<string, unknown>) => { savedState = structuredClone(value); } }),
  });
  const deliver = (data: unknown) => windowListeners.forEach(l => l(new FakeEvent('message', { data })));
  const list = doc.getElementById('values')!;
  const row = (p: string) => list.children.find(item => item.getAttribute('aria-label')?.startsWith(p + ','))!;
  const writes = () => posted.filter(m => m.type === 'write');
  return { doc, posted, deliver, list, row, writes, flushTimers: () => { const pending = [...timers.values()]; timers.clear(); pending.forEach(fn => fn()); }, savedState: () => savedState };
}

const setpoint = (number: number) => ({ path: 'tune_lab::SETPOINT', type: 'f32', kind: 'param', writable: true, display: `tune_lab::SETPOINT = ${number}`, number, reason: 'The firmware applies its range and step limits.' });
const speed = { path: 'motor.speed', type: 'f32', kind: 'watch', writable: false, display: 'motor.speed = 99.9 rad/s', number: 99.9, reason: 'watch! publishes a read-only snapshot.' };
const poll = (number: number) => ({ type: 'values', message: '', values: [setpoint(number), speed], watches: ['missing'], errors: [{ path: 'missing', error: 'symbol `missing` not found in the ELF' }] });

test('inline edits submit once on Enter, survive polls, and show the value read back', () => {
  const { doc, deliver, list, row, writes } = loadWebview();
  deliver(poll(100));
  assert.equal(list.children.length, 3);
  const item = row('tune_lab::SETPOINT');
  item.byClass('value')[0].click();
  const input = item.byClass('edit')[0];
  assert.ok(input, 'clicking a writable value opens the inline editor');
  assert.equal(input.value, '100');
  assert.equal(doc.activeElement, input);
  input.value = '250';
  deliver(poll(101));
  assert.equal(item.byClass('edit')[0], input, 'a poll keeps the open editor');
  assert.equal(input.value, '250', 'a poll does not overwrite typed input');

  input.key('Enter');
  input.key('Enter');
  assert.deepEqual(writes(), [{ type: 'write', path: 'tune_lab::SETPOINT', input: '250' }], 'a pending write blocks a second Enter');
  assert.ok(item.classes.has('pending'));

  deliver({ type: 'reply', path: 'tune_lab::SETPOINT', ok: false, text: '`250` is above the maximum 200' });
  assert.equal(item.byClass('edit')[0], input, 'a failed write leaves the editor open to retry');
  assert.equal(input.getAttribute('aria-invalid'), 'true');
  assert.equal(item.byClass('message')[0].textContent, '`250` is above the maximum 200');
  input.value = '150';
  input.key('Enter');
  assert.equal(writes().length, 2, 'the error is recoverable');

  deliver({ type: 'reply', path: 'tune_lab::SETPOINT', ok: true, text: 'Write sent.' });
  assert.equal(item.byClass('edit').length, 0, 'a successful write closes the editor');
  assert.equal(item.byClass('value')[0].textContent, '101', 'the row keeps the last read, not the typed value');
  assert.ok(item.classes.has('settling'));
  deliver(poll(150));
  assert.equal(item.byClass('value')[0].textContent, '150');
  assert.ok(!item.classes.has('settling'));
});

test('Escape and leaving the field cancel without writing; there are no Apply or Cancel buttons', () => {
  const { deliver, row, writes } = loadWebview();
  deliver(poll(100));
  const item = row('tune_lab::SETPOINT');
  item.focus();
  item.key('F2');
  const input = item.byClass('edit')[0];
  assert.ok(input);
  assert.equal(item.find(e => e.tagName === 'button' && /Apply|Cancel/.test(e.textContent)).length, 0);
  input.value = '5';
  input.key('Escape');
  assert.equal(item.byClass('edit').length, 0);
  item.key('Enter');
  item.byClass('edit')[0].value = '6';
  row('motor.speed').focus();
  assert.equal(item.byClass('edit').length, 0, 'blur cancels');
  assert.equal(writes().length, 0);
});

test('read-only and failed rows never edit, and explain themselves on demand', () => {
  const { doc, deliver, row, writes, posted } = loadWebview();
  deliver(poll(100));
  const readOnly = row('motor.speed');
  assert.ok(readOnly.classes.has('readonly'));
  readOnly.byClass('value')[0].click();
  assert.equal(readOnly.byClass('edit').length, 0);
  let popup = doc.body.byClass('reason-popup')[0];
  assert.ok(popup, 'clicking a read-only value immediately opens its reason');
  assert.match(popup.byClass('reason-text')[0].textContent, /read-only snapshot/);
  assert.equal(readOnly.byClass('detail')[0].hidden, true, 'the popup does not expand the list');
  assert.equal(doc.activeElement, popup);
  popup.key('Escape');
  assert.equal(doc.body.byClass('reason-popup').length, 0);
  assert.equal(doc.activeElement, readOnly);
  readOnly.key('Enter');
  popup = doc.body.byClass('reason-popup')[0];
  popup.byClass('reason-action')[0].click();
  assert.deepEqual(posted.at(-1), { type: 'suggest', path: 'motor.speed', input: '' });
  assert.equal(doc.body.byClass('reason-popup').length, 0);
  readOnly.key('F2');
  assert.ok(doc.body.byClass('reason-popup')[0], 'F2 explains a blocked edit too');
  doc.body.byClass('reason-popup')[0].key('Escape');

  readOnly.key('F10', { shiftKey: true });
  const menu = doc.body.byClass('menu')[0];
  const labels = menu.byClass('label').map(b => b.textContent);
  assert.ok(labels.includes('Suggest watch_mut! in Source'));
  assert.ok(!labels.includes('Edit Value'));
  assert.equal(doc.activeElement, menu.byClass('menu-item')[0], 'the menu takes keyboard focus');
  menu.byClass('menu-item').find(b => b.byClass('label')[0].textContent === 'Suggest watch_mut! in Source')!.click();
  assert.deepEqual(posted.at(-1), { type: 'suggest', path: 'motor.speed', input: '' });
  assert.equal(doc.body.byClass('menu').length, 0);
  assert.equal(doc.activeElement, readOnly, 'closing the menu returns focus to the row');

  const failed = row('missing');
  assert.ok(failed.classes.has('error'));
  failed.byClass('value')[0].click();
  const failedPopup = doc.body.byClass('reason-popup')[0];
  assert.match(failedPopup.byClass('reason-text')[0].textContent, /not found in the ELF/);
  assert.equal(failedPopup.byClass('reason-action').length, 0, 'unresolved locations do not offer a misleading source fix');
  failedPopup.key('Escape');
  failed.dispatch('contextmenu', { clientX: 10, clientY: 10 });
  const errorMenu = doc.body.byClass('menu')[0].byClass('label').map(b => b.textContent);
  assert.ok(errorMenu.includes('Show Error') && errorMenu.includes('Remove Watch') && !errorMenu.includes('Suggest watch_mut! in Source'));
  doc.body.byClass('menu')[0].key('Escape');
  failed.key('Delete');
  assert.deepEqual(posted.at(-1), { type: 'remove', path: 'missing', input: '' });

  const writable = row('tune_lab::SETPOINT');
  writable.key('ContextMenu');
  assert.ok(!doc.body.byClass('menu-item').some(b => b.textContent.startsWith('Suggest')));
  assert.equal(writes().length, 0);
});

test('the host never writes a read-only value and drops a duplicate write', async () => {
  const exports: Record<string, unknown> = {};
  let handler: (m: unknown) => Promise<void> = async () => {};
  const replies: unknown[] = [];
  const saved = new Map<string, unknown>();
  const workspaceState = { get: (key: string, fallback: unknown) => saved.get(key) ?? fallback, update: async (key: string, value: unknown) => { saved.set(key, structuredClone(value)); } };
  const disposable = { dispose() {} };
  const vscode = {
    Uri: { joinPath: (...parts: unknown[]) => parts.join('/') },
    EventEmitter: class { event = () => disposable; fire() {} dispose() {} },
    env: { clipboard: { writeText: async () => {} } },
  };
  runInNewContext(readFileSync(path.join(__dirname, '../watchPanel.js'), 'utf8'), {
    exports,
    require: (name: string) => name === 'vscode' ? vscode : require(name.startsWith('./') ? path.join(__dirname, '..', name) : name),
  });
  const values = [setpoint(100), speed, { ...speed, path: 'BOXED', expandable: true }].map(({ reason: _, ...v }) => v);
  const removed: string[] = [];
  const expansions: [string, boolean][] = [];
  const watch = { declared: values, watchedValues: new Map(), watched: ['missing'], lookup: (p: string) => values.find(v => v.path === p), removeWatch: async (p: string) => { removed.push(p); }, setExpanded: async (p: string, value: boolean) => { expansions.push([p, value]); } };
  const writes: string[] = [];
  let finish: () => void = () => {};
  const suggested: string[] = [];
  const Panel = exports.WatchPanel as new (...args: unknown[]) => { resolveWebviewView(view: unknown): void };
  const panel = new Panel('ext', watch, (value: { path: string }, input: string) => {
    writes.push(`${value.path}=${input}`);
    return new Promise(resolve => { finish = () => resolve({ ok: true, text: 'sent' }); });
  }, (p: string) => suggested.push(p), workspaceState);
  const view = {
    visible: true,
    onDidChangeVisibility: () => disposable,
    onDidDispose: () => disposable,
    webview: {
      cspSource: 'vscode-resource:', html: '', options: {},
      asWebviewUri: (u: string) => u,
      onDidReceiveMessage: (fn: typeof handler) => { handler = fn; return disposable; },
      postMessage: async (m: { type: string }) => { if (m.type === 'reply') replies.push(structuredClone(m)); return true; },
    },
  };
  panel.resolveWebviewView(view);
  assert.match(view.webview.html, /script-src 'nonce-/);
  assert.doesNotMatch(view.webview.html, /https?:/);
  assert.doesNotMatch(view.webview.html, />(Apply|Add watch)</);

  await handler({ type: 'expand', path: 'BOXED', input: 'true' });
  await handler({ type: 'expand', path: 'motor.speed', input: 'true' });
  await handler({ type: 'expand', path: 'unknown', input: 'true' });
  await handler({ type: 'expand', path: 'BOXED', input: 'invalid' });
  assert.deepEqual(expansions, [['BOXED', true]]);
  await handler({ type: 'write', path: 'motor.speed', input: '1' });
  assert.deepEqual(writes, []);
  assert.deepEqual(replies, [{ type: 'reply', path: 'motor.speed', ok: false, text: 'watch! publishes a read-only snapshot. Use watch_mut! with a mutable scalar to accept edits.' }]);

  const first = handler({ type: 'write', path: 'tune_lab::SETPOINT', input: ' 5 ' });
  await handler({ type: 'write', path: 'tune_lab::SETPOINT', input: '6' });
  finish();
  await first;
  assert.deepEqual(writes, ['tune_lab::SETPOINT=5']);
  assert.equal(replies.length, 2);

  await handler({ type: 'suggest', path: 'tune_lab::SETPOINT' });
  await handler({ type: 'suggest', path: 'motor.speed' });
  assert.deepEqual(suggested, ['motor.speed']);
  await handler({ type: 'remove', path: 'motor.speed' });
  await handler({ type: 'remove', path: 'missing' });
  assert.deepEqual(removed, ['missing']);

  await handler({ type: 'lock', path: 'tune_lab::SETPOINT', input: 'true' });
  assert.deepEqual(saved.get('tuningStudio.lockedValues'), ['tune_lab::SETPOINT']);
  await handler({ type: 'write', path: 'tune_lab::SETPOINT', input: '9' });
  assert.equal(writes.length, 1, 'a user lock is enforced by the host');
  assert.match((replies.at(-1) as { text: string }).text, /Locked by you/);
  // Recreate the provider using the saved workspace state.
  let reopenedWrites = 0;
  const reopened = new Panel('ext', watch, async () => { reopenedWrites++; return { ok: true, text: 'sent' }; }, () => {}, workspaceState);
  reopened.resolveWebviewView(view);
  await handler({ type: 'write', path: 'tune_lab::SETPOINT', input: '9' });
  assert.equal(reopenedWrites, 0, 'the lock survives provider recreation');
  await handler({ type: 'lock', path: 'tune_lab::SETPOINT', input: 'false' });
  await handler({ type: 'write', path: 'tune_lab::SETPOINT', input: '9' });
  assert.equal(reopenedWrites, 1);
  await handler({ type: 'lock', path: 'motor.speed', input: 'false' });
  await handler({ type: 'write', path: 'motor.speed', input: '9' });
  assert.equal(reopenedWrites, 1, 'unlocking never overrides backend restrictions');
});


test('structs expand with short field names, keyboard navigation, and persistent expansion', () => {
  const values = [{ path: 'motor.state', type: 'MotorSnapshot', kind: 'watch', writable: false, display: 'motor.state: MotorSnapshot', children: [
    { ...speed, path: 'motor.state.speed', label: 'speed' },
    { path: 'motor.state.pi', label: 'pi', type: 'PiSnapshot', writable: false, display: 'motor.state.pi: PiSnapshot', children: [
      { ...speed, path: 'motor.state.pi.integral', label: 'integral' },
    ] },
  ] }, setpoint(100)];
  const payload = { type: 'values', message: '', values, watches: [], errors: [] };
  const ui = loadWebview();
  ui.deliver(payload);
  const root = ui.row('motor.state');
  const child = ui.row('motor.state.speed');
  const nested = ui.row('motor.state.pi');
  const integral = ui.row('motor.state.pi.integral');
  assert.equal(root.getAttribute('role'), 'treeitem');
  assert.equal(root.getAttribute('aria-expanded'), 'false');
  assert.equal(child.hidden, true);
  root.focus(); root.key('ArrowDown');
  assert.equal(ui.doc.activeElement, ui.row('tune_lab::SETPOINT'), 'navigation skips collapsed descendants');
  root.byClass('toggle')[0].click();
  assert.equal(child.hidden, false);
  assert.equal(child.byClass('leaf')[0].textContent, 'speed');
  assert.equal(child.getAttribute('aria-level'), '2');
  assert.equal(integral.hidden, true);
  nested.focus(); nested.key('ArrowRight');
  assert.equal(integral.hidden, false);
  integral.focus(); integral.key('ArrowLeft');
  assert.equal(ui.doc.activeElement, nested);
  root.byClass('toggle')[0].click();
  assert.equal(integral.hidden, true);
  root.byClass('toggle')[0].click();
  assert.equal(integral.hidden, false, 'nested expansion survives collapsing its ancestor');
  ui.deliver(payload);
  assert.equal(ui.row('motor.state'), root, 'polls retain the same rows');
  assert.equal(integral.hidden, false);
  const reopened = loadWebview(ui.savedState());
  reopened.deliver(payload);
  assert.equal(reopened.row('motor.state.pi.integral').hidden, false);
});

test('writable struct fields keep inline edits through polls; collapsing cancels without writing', () => {
  const payload = { type: 'values', message: '', values: [{ path: 'CONTROL', type: 'Controller', writable: false, display: 'CONTROL: Controller', children: [
    { ...setpoint(2), path: 'CONTROL.kp', label: 'kp', kind: 'raw' },
  ] }], watches: ['CONTROL'], errors: [] };
  const ui = loadWebview();
  ui.deliver(payload);
  ui.row('CONTROL').key('ArrowRight');
  const child = ui.row('CONTROL.kp');
  child.byClass('value')[0].click();
  const input = child.byClass('edit')[0];
  input.value = '0.5';
  ui.deliver(payload);
  assert.equal(child.byClass('edit')[0], input);
  assert.equal(input.value, '0.5');
  input.key('Enter');
  assert.deepEqual(ui.writes(), [{ type: 'write', path: 'CONTROL.kp', input: '0.5' }]);
  ui.deliver({ type: 'reply', path: 'CONTROL.kp', ok: true, text: 'Applied' });
  child.byClass('value')[0].click();
  ui.row('CONTROL').byClass('toggle')[0].click();
  assert.equal(child.hidden, true);
  assert.equal(child.byClass('edit').length, 0);
  assert.equal(ui.writes().length, 1);
});


test('pointer arrows request pointees lazily and restore their open state', () => {
  const pointer = { path: 'BOXED', type: 'Box<Controller>', kind: 'raw', writable: false, display: 'BOXED: Box<Controller> = 0x20000000', expandable: true };
  const initial = { type: 'values', message: '', values: [pointer], watches: ['BOXED'], errors: [] };
  const loaded = { ...initial, values: [{ ...pointer, children: [{ path: 'BOXED.*', label: '*', type: 'Controller', kind: 'raw', writable: false, display: 'BOXED.*: Controller', children: [
    { ...setpoint(2), path: 'BOXED.*.kp', label: 'kp', kind: 'raw' },
  ] }] }] };
  const ui = loadWebview();
  ui.deliver(initial);
  assert.equal(ui.row('BOXED').getAttribute('aria-expanded'), 'false');
  assert.equal(ui.posted.filter(m => m.type === 'expand').length, 0);
  ui.row('BOXED').key('ArrowRight');
  assert.deepEqual(ui.posted.filter(m => m.type === 'expand'), [{ type: 'expand', path: 'BOXED', input: 'true' }]);
  ui.deliver(loaded);
  assert.equal(ui.row('BOXED.*').hidden, false);
  assert.equal(ui.row('BOXED.*.kp').hidden, true);
  ui.row('BOXED.*').key('ArrowRight');
  assert.equal(ui.row('BOXED.*.kp').hidden, false);
  assert.equal(ui.posted.filter(m => m.type === 'expand').length, 1, 'inline struct expansion needs no extra read');
  const reopened = loadWebview(ui.savedState());
  reopened.deliver(initial); reopened.deliver(initial);
  assert.equal(reopened.posted.filter(m => m.type === 'expand').length, 1, 'restored pointer expansion is requested once');
  reopened.deliver({ type: 'expansionReply', path: 'BOXED', ok: false, text: 'Pointer expansion limit (64)' });
  assert.equal(reopened.row('BOXED').getAttribute('aria-expanded'), 'false');
  assert.match(reopened.row('BOXED').byClass('detail')[0].textContent, /limit \(64\)/);
  ui.row('BOXED').key('ArrowLeft');
  assert.equal(ui.row('BOXED.*').hidden, true);
  assert.deepEqual(ui.posted.filter(m => m.type === 'expand').at(-1), { type: 'expand', path: 'BOXED', input: 'false' });
  ui.deliver(initial);
  assert.equal(ui.posted.filter(m => m.type === 'expand').length, 2);
});


test('watch suggestions retain focus, ignore stale replies, and accept before adding', () => {
  const { doc, posted, deliver, flushTimers } = loadWebview();
  const input = doc.getElementById('location')!;
  deliver({ type: 'addFocus' });
  assert.equal(doc.activeElement, input, 'the title action uses the same inline field');
  const old = posted.at(-1)!;
  input.value = 'gim'; input.dispatch('input'); flushTimers();
  const current = posted.at(-1)!;
  assert.equal(current.type, 'complete');
  assert.equal(current.query, 'gim');
  deliver({ type: 'completions', id: old.id, query: '', items: [{ path: 'stale', type: 'u32' }] });
  assert.equal(doc.getElementById('watch-suggestions')!.hidden, true);
  deliver({ type: 'completions', id: current.id, query: 'gim', items: [{ path: 'sentry::GIMBAL', type: 'StaticCell<Pid>' }] });
  assert.equal(doc.getElementById('watch-suggestions')!.hidden, false);
  assert.equal(doc.activeElement, input);
  input.key('ArrowDown');
  assert.equal(input.getAttribute('aria-activedescendant'), 'suggestion-0');
  assert.equal(input.key('Enter').defaultPrevented, true);
  assert.equal(input.value, 'sentry::GIMBAL');
  assert.equal(posted.some(m => m.type === 'add'), false);
  doc.getElementById('add')!.dispatch('submit');
  assert.equal(posted.at(-1)?.type, 'add');
  assert.equal(posted.at(-1)?.path, 'sentry::GIMBAL');
  assert.equal(input.value, '');

  input.value = '0x20000000:f32'; input.dispatch('input');
  input.key('Escape'); flushTimers();
  deliver({ type: 'completions', id: current.id, query: 'gim', items: [] });
  assert.equal(doc.getElementById('watch-suggestions')!.hidden, true);
  assert.equal(input.value, '0x20000000:f32', 'dismissing suggestions preserves manual input');
  input.dispatch('input'); flushTimers();
  input.key('Escape');
  assert.equal(input.value, '0x20000000:f32', 'Escape also dismisses an in-flight request without clearing input');
  doc.getElementById('add')!.dispatch('submit');
  assert.equal(posted.at(-1)?.path, '0x20000000:f32');
});

test('user locks use the existing context menu without suggesting firmware changes', () => {
  const { doc, posted, deliver, row, writes } = loadWebview();
  const value = { ...setpoint(100), lockable: true };
  deliver({ type: 'values', message: '', values: [value], watches: [], errors: [] });
  row(value.path).key('ContextMenu');
  const lock = doc.body.byClass('menu-item').find(item => item.textContent === 'Lock Value')!;
  assert.ok(lock); lock.click();
  assert.deepEqual(posted.at(-1), { type: 'lock', path: value.path, input: 'true' });
  deliver({ type: 'values', message: '', values: [{ ...value, writable: false, userLocked: true, reason: 'Locked by you.' }], watches: [], errors: [] });
  row(value.path).byClass('value')[0].click();
  assert.equal(row(value.path).byClass('edit').length, 0);
  assert.equal(doc.body.byClass('reason-action').length, 0);
  doc.body.byClass('reason-popup')[0].key('Escape');
  row(value.path).key('ContextMenu');
  const unlock = doc.body.byClass('menu-item').find(item => item.textContent === 'Unlock Value')!;
  assert.ok(unlock); unlock.click();
  assert.deepEqual(posted.at(-1), { type: 'lock', path: value.path, input: 'false' });
  assert.equal(writes().length, 0);
});


test('suggestion requests coalesce while a CLI call is in flight', async () => {
  let handler: (message: unknown) => Promise<void> = async () => {};
  const posted: Record<string, unknown>[] = [];
  const calls: string[] = [];
  const finish: (() => void)[] = [];
  const disposable = { dispose() {} };
  const exports: Record<string, unknown> = {};
  runInNewContext(readFileSync(path.join(__dirname, '../watchPanel.js'), 'utf8'), {
    exports,
    require: (name: string) => name === 'vscode' ? {
      Uri: { joinPath: (...parts: unknown[]) => parts.join('/') },
      EventEmitter: class { event = () => disposable; fire() {} dispose() {} },
    } : require(name.startsWith('./') ? path.join(__dirname, '..', name) : name),
  });
  const Panel = exports.WatchPanel as new (...args: unknown[]) => { resolveWebviewView(view: unknown): void };
  const panel = new Panel('ext', { declared: [], watchedValues: new Map(), watched: [] }, async () => {}, () => {}, undefined, (query: string) => {
    calls.push(query);
    return new Promise(resolve => finish.push(() => resolve({ items: [{ path: query, label: query, type: 'u32' }] })));
  });
  panel.resolveWebviewView({ visible: true, onDidChangeVisibility: () => disposable, onDidDispose: () => disposable,
    webview: { cspSource: 'local', asWebviewUri: (uri: string) => uri,
      onDidReceiveMessage: (fn: typeof handler) => { handler = fn; return disposable; },
      postMessage: async (message: Record<string, unknown>) => { posted.push(structuredClone(message)); },
    },
  });
  await handler({ type: 'complete', query: 'a', id: 1 });
  await handler({ type: 'complete', query: 'ab', id: 2 });
  await handler({ type: 'complete', query: 'abc', id: 3 });
  assert.deepEqual(calls, ['a']);
  finish[0](); await new Promise<void>(resolve => setImmediate(resolve));
  assert.deepEqual(calls, ['a', 'abc']);
  assert.equal(posted.some(message => message.type === 'completions'), false, 'superseded results are not posted');
  finish[1](); await new Promise<void>(resolve => setImmediate(resolve));
  const reply = posted.find(message => message.type === 'completions')!;
  assert.equal(reply.id, 3); assert.equal(reply.query, 'abc');
});

test('watch names drag their full paths', () => {
  const h = loadWebview(); h.deliver(poll(1));
  const name = h.row('motor.speed').byClass('name')[0];
  const payload = new Map<string, string>();
  const transfer = { setData: (type: string, value: string) => payload.set(type, value), effectAllowed: '' };
  name.dispatch('dragstart', { dataTransfer: transfer });
  assert.equal(payload.get('application/x-tuning-studio-watch'), 'motor.speed');
  assert.equal(payload.get('text/plain'), 'motor.speed');
  assert.equal(transfer.effectAllowed, 'copy');
});

test('horizontal scrolling inside a long watch input does not cancel suggestions', () => {
  const h = loadWebview();
  const input = h.doc.getElementById('location')!;
  input.focus(); input.value = 'hku_target_vehicle_board::control::POOL[task 0].future.';
  input.dispatch('input'); input.dispatch('scroll'); h.flushTimers();
  const request = h.posted.filter(m => m.type === 'complete').at(-1)!;
  assert.equal(request.query, input.value);
  h.deliver({ type: 'completions', id: request.id, query: input.value, items: [{ path: input.value + '#3', label: 'Suspend0', type: 'Suspend0' }] });
  const suggestions = h.doc.getElementById('watch-suggestions')!;
  assert.equal(suggestions.hidden, false);
  input.dispatch('scroll'); assert.equal(suggestions.hidden, false);
});
