// @ts-check
// Live Watch sidebar. One focusable row per value: arrow keys move between
// rows, Enter or F2 edits a writable value, Shift+F10 or the context menu key
// opens the row actions. Text only ever goes through textContent.
(function () {
  // @ts-ignore VS Code provides this API.
  const host = acquireVsCodeApi();
  const list = /** @type {HTMLElement} */ (document.getElementById('values'));
  const status = /** @type {HTMLElement} */ (document.getElementById('status'));
  const empty = /** @type {HTMLElement} */ (document.getElementById('empty'));
  const announce = /** @type {HTMLElement} */ (document.getElementById('announce'));
  const addForm = /** @type {HTMLFormElement} */ (document.getElementById('add'));
  const addInput = /** @type {HTMLInputElement} */ (document.getElementById('location'));

  /**
   * @typedef {{ text: string, full: string, reason: string, error: string, writable: boolean,
   *   suggest: boolean, number: number | undefined, watch: string | undefined }} Info
   * @typedef {{ path: string, item: HTMLElement, row: HTMLElement, value: HTMLElement,
   *   more: HTMLButtonElement, detail: HTMLElement, message: HTMLElement,
   *   input: HTMLInputElement | undefined, info: Info, pending: boolean, settling: boolean,
   *   writeError: string, showDetail: boolean }} Row
   */
  /** @type {Map<string, Row>} */
  const rows = new Map();
  /** @type {string[]} */
  let order = [];
  /** @type {string | undefined} */
  let active;
  /** @type {{ element: HTMLElement, row: Row } | undefined} */
  let menu;
  let ids = 0;

  /** @param {string} type @param {string} path @param {string} [input] */
  const send = (type, path, input = '') => host.postMessage({ type, path, input });
  /** @param {string} tag @param {string} [className] @param {string} [text] */
  function el(tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = text;
    return element;
  }
  /** @param {string} text */
  function say(text) { announce.textContent = ''; announce.textContent = text; }

  /** Split `crate::module::NAME` or `motor.speed` so the last segment stands out. */
  function nameParts(path) {
    const match = /^(.*(?:::|\.))([^.:]+.*)$/.exec(path);
    return match ? [match[1], match[2]] : ['', path];
  }

  /** The short value shown in the row; the full display goes in the tooltip. */
  function shortValue(value) {
    if (value.error) return value.error.split(/\r?\n/, 1)[0];
    const first = String(value.display).split(/\r?\n/, 1)[0];
    const equals = first.indexOf(' = ');
    if (equals >= 0) return first.slice(equals + 3).split(' (', 1)[0];
    return value.number !== undefined ? String(value.number) : String(value.type);
  }

  /** @param {string} path @param {string[]} watches */
  function watchOf(path, watches) {
    return watches.find(w => path === w || path.startsWith(w + '.') || path.startsWith(w + '['));
  }

  /** @param {string} path @returns {Row} */
  function makeRow(path) {
    const item = el('div', 'item');
    item.setAttribute('role', 'listitem');
    item.tabIndex = -1;
    const row = el('div', 'row');
    const name = el('span', 'name');
    const [prefix, leaf] = nameParts(path);
    name.append(el('span', 'prefix', prefix), el('span', 'leaf', leaf));
    const value = el('span', 'value');
    value.setAttribute('aria-hidden', 'true');
    const more = /** @type {HTMLButtonElement} */ (el('button', 'more', '⋯'));
    more.type = 'button';
    more.tabIndex = -1;
    more.setAttribute('aria-haspopup', 'menu');
    more.setAttribute('aria-label', `Actions for ${path}`);
    row.append(name, value, more);
    const message = el('div', 'message');
    message.hidden = true;
    const detail = el('div', 'detail');
    detail.id = `reason-${++ids}`;
    detail.hidden = true;
    item.setAttribute('aria-describedby', detail.id);
    item.append(row, message, detail);
    /** @type {Row} */
    const r = {
      path, item, row, value, more, detail, message, input: undefined, pending: false, settling: false, writeError: '', showDetail: false,
      info: { text: '', full: '', reason: '', error: '', writable: false, suggest: false, number: undefined, watch: undefined },
    };
    value.addEventListener('click', () => startEdit(r));
    more.addEventListener('click', event => { event.stopPropagation(); openMenu(r); });
    row.addEventListener('mousedown', () => { active = path; });
    item.addEventListener('focus', () => { active = path; updateTabStops(); });
    item.addEventListener('contextmenu', event => {
      event.preventDefault();
      const mouse = /** @type {MouseEvent} */ (event);
      item.focus();
      openMenu(r, mouse.clientX, mouse.clientY);
    });
    item.addEventListener('keydown', event => onRowKey(r, /** @type {KeyboardEvent} */ (event)));
    rows.set(path, r);
    return r;
  }

  /** @param {Row} r */
  function render(r) {
    const { info } = r;
    r.value.textContent = info.text;
    const failed = r.writeError ? `\nLast write failed: ${r.writeError}` : '';
    r.row.title = `${r.path}\n${info.full}${info.reason ? '\n' + info.reason : ''}${failed}`;
    r.item.classList.toggle('readonly', !info.writable && !info.error);
    r.item.classList.toggle('error', !!info.error);
    r.item.classList.toggle('writable', info.writable);
    r.item.classList.toggle('settling', r.settling);
    r.item.classList.toggle('failed', !!r.writeError);
    const state = info.error ? 'unavailable' : info.writable ? 'editable' : 'read-only';
    r.item.setAttribute('aria-label', `${r.path}, ${info.text}, ${state}${r.writeError ? ', last write failed' : ''}`);
    r.detail.textContent = [info.error || info.reason, r.writeError && `Last write failed: ${r.writeError}`].filter(Boolean).join('\n');
    r.detail.hidden = !r.showDetail;
  }

  /** @param {Row} r */
  function startEdit(r) {
    if (!r.info.writable) { openReason(r); return; }
    if (r.pending) return;
    if (r.input) { r.input.focus(); return; }
    closeMenu(false);
    const input = /** @type {HTMLInputElement} */ (el('input', 'edit'));
    input.value = r.info.number !== undefined ? String(r.info.number) : r.info.text;
    input.setAttribute('aria-label', `New value for ${r.path}. Enter writes, Escape cancels.`);
    input.spellcheck = false;
    input.addEventListener('keydown', event => {
      const key = /** @type {KeyboardEvent} */ (event);
      event.stopPropagation();
      if (key.key === 'Enter') { event.preventDefault(); submit(r); }
      else if (key.key === 'Escape') { event.preventDefault(); closeEditor(r, true); }
    });
    input.addEventListener('input', () => { r.message.hidden = true; input.removeAttribute('aria-invalid'); });
    // Leaving the field cancels, unless the whole window lost focus.
    input.addEventListener('blur', () => { if (r.input === input && !r.pending && document.hasFocus()) closeEditor(r, false); });
    r.input = input;
    r.writeError = '';
    r.value.hidden = true;
    r.row.insertBefore(input, r.more);
    r.item.classList.add('editing');
    render(r);
    input.focus();
    input.select();
  }

  /** @param {Row} r */
  function submit(r) {
    const input = r.input;
    if (!input || r.pending || !r.info.writable) return;
    const text = input.value.trim();
    if (!text) return;
    r.pending = true;
    input.readOnly = true;
    input.removeAttribute('aria-invalid');
    r.message.hidden = true;
    r.item.classList.add('pending');
    r.item.setAttribute('aria-busy', 'true');
    send('write', r.path, text);
  }

  /** @param {Row} r @param {boolean} refocus */
  function closeEditor(r, refocus) {
    const input = r.input;
    if (!input) return;
    r.input = undefined;
    r.message.hidden = true;
    r.value.hidden = false;
    r.item.classList.remove('editing');
    input.remove();
    if (refocus) r.item.focus();
  }

  /** @param {Row} r @param {{ ok?: unknown, text?: unknown }} reply */
  function onReply(r, reply) {
    const text = typeof reply.text === 'string' ? reply.text : '';
    r.pending = false;
    r.item.classList.remove('pending');
    r.item.removeAttribute('aria-busy');
    if (reply.ok === true) {
      // The value shown changes only when a later poll reads it back.
      r.writeError = '';
      r.settling = true;
      const focused = !!r.input && document.activeElement === r.input;
      closeEditor(r, focused);
      say(`Write sent for ${r.path}`);
    } else {
      r.writeError = text || 'Write failed';
      if (r.input) {
        r.input.readOnly = false;
        r.input.setAttribute('aria-invalid', 'true');
        r.message.textContent = r.writeError;
        r.message.title = r.writeError;
        r.message.hidden = false;
      }
      say(`Write failed for ${r.path}: ${r.writeError}`);
    }
    render(r);
  }

  /** @param {Row} r @param {number} step */
  function move(r, step) {
    const index = order.indexOf(r.path);
    const next = step === -Infinity ? 0 : step === Infinity ? order.length - 1 : Math.min(order.length - 1, Math.max(0, index + step));
    rows.get(order[next])?.item.focus();
  }

  /** @param {Row} r @param {KeyboardEvent} event */
  function onRowKey(r, event) {
    if (event.target !== r.item) return;
    const mod = event.metaKey || event.ctrlKey;
    let handled = true;
    if (event.key === 'ArrowDown') move(r, 1);
    else if (event.key === 'ArrowUp') move(r, -1);
    else if (event.key === 'Home') move(r, -Infinity);
    else if (event.key === 'End') move(r, Infinity);
    else if (event.key === 'Enter' || event.key === 'F2') startEdit(r);
    else if (event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) openMenu(r);
    else if (event.key === 'Escape' && r.showDetail) toggleDetail(r);
    else if ((event.key === 'Delete' || (event.key === 'Backspace' && event.metaKey)) && r.info.watch) send('remove', r.info.watch);
    else if (mod && event.key.toLowerCase() === 'c') send('copy', r.path, 'value');
    else handled = false;
    if (handled) event.preventDefault();
  }

  /** @param {Row} r */
  function toggleDetail(r) { r.showDetail = !r.showDetail; render(r); }

  /** @param {Row} r */
  function menuItems(r) {
    const { info } = r;
    /** @type {{ label: string, key?: string, run: () => void }[][]} */
    const groups = [[], [], []];
    if (info.writable) groups[0].push({ label: 'Edit Value', key: 'Enter', run: () => startEdit(r) });
    if (info.error || info.reason || r.writeError) {
      const noun = info.error || r.writeError ? 'Error' : 'Reason';
      groups[0].push({ label: `${r.showDetail ? 'Hide' : 'Show'} ${noun}`, key: info.writable ? undefined : 'Enter', run: () => toggleDetail(r) });
    }
    if (info.suggest) groups[0].push({ label: 'Suggest watch_mut! in Source', run: () => send('suggest', r.path) });
    if (!info.error) groups[1].push({ label: 'Copy Value', key: '⌘C', run: () => send('copy', r.path, 'value') });
    groups[1].push({ label: 'Copy Path', run: () => send('copy', r.path, 'path') });
    if (info.watch) {
      const watch = info.watch;
      groups[2].push({ label: watch === r.path ? 'Remove Watch' : `Remove Watch ${watch}`, key: 'Delete', run: () => send('remove', watch) });
    }
    return groups.filter(group => group.length);
  }

  /** @param {Row} r @param {number} [x] @param {number} [y] */
  function openMenu(r, x, y) {
    closeMenu(false);
    const element = el('div', 'menu');
    element.setAttribute('role', 'menu');
    element.setAttribute('aria-label', `Actions for ${r.path}`);
    /** @type {HTMLButtonElement[]} */
    const buttons = [];
    menuItems(r).forEach((group, index) => {
      if (index) { const line = el('div', 'separator'); line.setAttribute('role', 'separator'); element.append(line); }
      for (const entry of group) {
        const button = /** @type {HTMLButtonElement} */ (el('button', 'menu-item'));
        button.type = 'button';
        button.tabIndex = -1;
        button.setAttribute('role', 'menuitem');
        button.append(el('span', 'label', entry.label));
        if (entry.key) { const key = el('span', 'key', entry.key); key.setAttribute('aria-hidden', 'true'); button.append(key); }
        button.addEventListener('click', () => { closeMenu(true); entry.run(); });
        button.addEventListener('mouseenter', () => button.focus());
        buttons.push(button);
        element.append(button);
      }
    });
    element.addEventListener('keydown', event => {
      const key = /** @type {KeyboardEvent} */ (event).key;
      const index = buttons.indexOf(/** @type {HTMLButtonElement} */ (document.activeElement));
      let next = -1;
      if (key === 'ArrowDown') next = (index + 1) % buttons.length;
      else if (key === 'ArrowUp') next = (index - 1 + buttons.length) % buttons.length;
      else if (key === 'Home') next = 0;
      else if (key === 'End') next = buttons.length - 1;
      else if (key === 'Escape' || key === 'Tab') { event.preventDefault(); closeMenu(true); return; }
      else return;
      event.preventDefault();
      buttons[next].focus();
    });
    showPopup(r, element, r.more.getBoundingClientRect(), x, y);
    buttons[0]?.focus();
  }

  /** Explain a blocked edit at the value itself, without expanding the list. @param {Row} r */
  function openReason(r) {
    closeMenu(false);
    const element = el('div', 'menu reason-popup');
    element.setAttribute('role', 'dialog');
    element.setAttribute('aria-label', `Cannot edit ${r.path}`);
    element.tabIndex = -1;
    element.append(el('div', 'reason-title', r.info.error ? 'Value unavailable' : 'Read-only value'));
    element.append(el('div', 'reason-text', r.info.error || r.info.reason || 'This value cannot be edited.'));
    if (r.info.suggest) {
      const suggest = /** @type {HTMLButtonElement} */ (el('button', 'reason-action', 'Suggest watch_mut! in Source'));
      suggest.type = 'button';
      suggest.addEventListener('click', () => { closeMenu(true); send('suggest', r.path); });
      element.append(suggest);
    }
    element.addEventListener('keydown', event => {
      const key = /** @type {KeyboardEvent} */ (event).key;
      if (key === 'Escape') { event.preventDefault(); closeMenu(true); }
      else if (key === 'Tab' && (/** @type {KeyboardEvent} */ (event).shiftKey || !r.info.suggest || document.activeElement !== element)) {
        // Let Tab reach the suggestion, then return to the row when leaving.
        event.preventDefault(); closeMenu(true);
      }
    });
    showPopup(r, element, r.value.getBoundingClientRect());
    element.focus();
  }

  /** @param {Row} r @param {HTMLElement} element @param {DOMRect} anchor @param {number} [x] @param {number} [y] */
  function showPopup(r, element, anchor, x, y) {
    document.body.append(element);
    menu = { element, row: r };
    r.item.classList.add('menu-open');
    r.more.setAttribute('aria-expanded', 'true');
    const left = x ?? anchor.right;
    const top = y ?? anchor.bottom;
    const box = element.getBoundingClientRect();
    const width = window.innerWidth || box.width;
    const height = window.innerHeight || box.height;
    element.style.left = `${Math.max(0, Math.min(left - (x === undefined ? box.width : 0), width - box.width))}px`;
    element.style.top = `${Math.max(0, top + box.height > height ? top - box.height : top)}px`;
  }

  /** @param {boolean} refocus */
  function closeMenu(refocus) {
    if (!menu) return;
    const { element, row } = menu;
    menu = undefined;
    row.item.classList.remove('menu-open');
    row.more.setAttribute('aria-expanded', 'false');
    element.remove();
    if (refocus && rows.get(row.path) === row) row.item.focus();
  }

  function updateTabStops() {
    if (active === undefined || !rows.has(active)) active = order[0];
    for (const [path, r] of rows) r.item.tabIndex = path === active ? 0 : -1;
  }

  /** @param {any} m */
  function onValues(m) {
    status.textContent = typeof m.message === 'string' ? m.message : '';
    status.title = status.textContent;
    status.hidden = !status.textContent;
    const watches = m.watches.filter(w => typeof w === 'string');
    /** @type {Map<string, Info>} */
    const next = new Map();
    for (const value of m.values) {
      if (!value || typeof value.path !== 'string' || next.has(value.path)) continue;
      const error = typeof value.error === 'string' ? value.error : '';
      next.set(value.path, {
        text: shortValue(value),
        full: String(value.display ?? ''),
        reason: typeof value.reason === 'string' && value.reason !== error ? value.reason : '',
        error,
        writable: value.writable === true && !error,
        suggest: value.writable !== true && !error,
        number: typeof value.number === 'number' ? value.number : undefined,
        watch: watchOf(value.path, watches),
      });
    }
    for (const entry of m.errors) {
      if (!entry || typeof entry.path !== 'string' || !entry.error || next.has(entry.path)) continue;
      next.set(entry.path, { text: entry.error, full: '', reason: '', error: entry.error, writable: false, suggest: false, number: undefined, watch: entry.path });
    }
    for (const [path, r] of rows) {
      if (next.has(path)) continue;
      if (menu?.row === r) closeMenu(false);
      r.item.remove();
      rows.delete(path);
    }
    const paths = [...next.keys()];
    const reorder = paths.length !== order.length || paths.some((p, i) => p !== order[i]);
    for (const [path, info] of next) {
      const r = rows.get(path) ?? makeRow(path);
      // A poll after a successful write shows the value read back.
      r.settling = false;
      r.info = info;
      if (r.input && !info.writable && !r.pending) closeEditor(r, true);
      render(r);
    }
    // Re-appending moves nodes, which would blur an open editor; only do it when the order changed.
    if (reorder) {
      const focused = document.activeElement;
      for (const path of paths) list.append(/** @type {Row} */ (rows.get(path)).item);
      if (focused && focused !== document.activeElement) /** @type {HTMLElement} */ (focused).focus();
    }
    order = paths;
    empty.hidden = order.length > 0;
    updateTabStops();
  }

  window.addEventListener('message', event => {
    const m = event.data;
    if (!m || typeof m !== 'object') return;
    if (m.type === 'reply') { const r = rows.get(m.path); if (r && r.pending) onReply(r, m); return; }
    if (m.type === 'edit') { const r = rows.get(m.path); if (r) { r.item.focus(); startEdit(r); } return; }
    if (m.type !== 'values' || !Array.isArray(m.values) || !Array.isArray(m.watches) || !Array.isArray(m.errors)) return;
    onValues(m);
  });
  document.addEventListener('mousedown', event => {
    if (menu && !menu.element.contains(/** @type {Node} */ (event.target))) closeMenu(false);
  });
  window.addEventListener('blur', () => closeMenu(false));
  window.addEventListener('resize', () => closeMenu(false));
  window.addEventListener('scroll', () => closeMenu(false), true);
  addForm.addEventListener('submit', event => {
    event.preventDefault();
    const location = addInput.value.trim();
    if (location) { send('add', location); addInput.value = ''; }
  });
  addInput.addEventListener('keydown', event => {
    if (/** @type {KeyboardEvent} */ (event).key === 'Escape') { addInput.value = ''; addInput.blur(); }
  });
  host.postMessage({ type: 'ready' });
})();
