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
   *   suggest: boolean, lockable: boolean, userLocked: boolean, number: number | undefined, watch: string | undefined, depth: number, parent: string | undefined,
   *   label: string | undefined, expandable: boolean, indirect: boolean }} Info
   * @typedef {{ path: string, item: HTMLElement, row: HTMLElement, value: HTMLElement,
   *   more: HTMLButtonElement, toggle: HTMLButtonElement, name: HTMLElement, detail: HTMLElement, message: HTMLElement,
   *   input: HTMLInputElement | undefined, info: Info, pending: boolean, settling: boolean,
   *   writeError: string, expansionError: string, showDetail: boolean }} Row
   */
  /** @type {Map<string, Row>} */
  const rows = new Map();
  /** @type {string[]} */
  let order = [];
  /** @type {string[]} */
  let mounted = [];
  const saved = host.getState?.();
  /** @type {Set<string>} */
  const expanded = new Set(Array.isArray(saved?.expanded) ? saved.expanded.filter(p => typeof p === 'string').slice(0, 1000) : []);
  /** @type {string | undefined} */
  let active;
  /** @type {{ element: HTMLElement, row: Row } | undefined} */
  let menu;
  let ids = 0;
  const requested = new Set();

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
    item.setAttribute('role', 'treeitem');
    item.tabIndex = -1;
    const row = el('div', 'row');
    const toggle = /** @type {HTMLButtonElement} */ (el('button', 'toggle'));
    toggle.type = 'button'; toggle.tabIndex = -1;
    const name = el('span', 'name');
    const [prefix, leaf] = nameParts(path);
    const nameText = el('bdi'); nameText.dir = 'ltr';
    nameText.append(el('span', 'prefix', prefix), el('span', 'leaf', leaf)); name.append(nameText);
    name.draggable = true;
    name.title = path;
    name.addEventListener('dragstart', event => {
      if (r.info.error || r.info.expandable) { event.preventDefault(); return; }
      event.dataTransfer?.setData('application/x-tuning-studio-watch', path);
      event.dataTransfer?.setData('text/plain', path);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = 'copy';
    });
    const value = el('span', 'value');
    value.setAttribute('aria-hidden', 'true');
    const more = /** @type {HTMLButtonElement} */ (el('button', 'more', '⋯'));
    more.type = 'button';
    more.tabIndex = -1;
    more.setAttribute('aria-haspopup', 'menu');
    more.setAttribute('aria-label', `Actions for ${path}`);
    row.append(toggle, name, value, more);
    const message = el('div', 'message');
    message.hidden = true;
    const detail = el('div', 'detail');
    detail.id = `reason-${++ids}`;
    detail.hidden = true;
    item.setAttribute('aria-describedby', detail.id);
    item.append(row, message, detail);
    /** @type {Row} */
    const r = {
      path, item, row, value, more, toggle, name, detail, message, input: undefined, pending: false, settling: false, writeError: '', expansionError: '', showDetail: false,
      info: { text: '', full: '', reason: '', error: '', writable: false, suggest: false, lockable: false, userLocked: false, number: undefined, watch: undefined, depth: 0, parent: undefined, label: undefined, expandable: false, indirect: false },
    };
    value.addEventListener('click', () => startEdit(r));
    toggle.addEventListener('click', event => { event.stopPropagation(); toggleExpanded(r); });
    name.addEventListener('click', () => { if (r.info.expandable) toggleExpanded(r); });
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
    const [prefix, leaf] = info.depth > 0 ? ['', info.label ?? nameParts(r.path)[1]] : nameParts(r.path);
    const nameText = el('bdi'); nameText.dir = 'ltr';
    nameText.append(el('span', 'prefix', prefix), el('span', 'leaf', leaf)); r.name.replaceChildren(nameText);
    r.item.style.paddingLeft = `${info.depth * 12}px`;
    r.item.setAttribute('aria-level', String(info.depth + 1));
    r.toggle.style.visibility = info.expandable ? 'visible' : 'hidden';
    r.toggle.textContent = expanded.has(r.path) ? '⌄' : '›';
    r.toggle.setAttribute('aria-label', `${expanded.has(r.path) ? 'Collapse' : 'Expand'} ${r.path}`);
    if (info.expandable) r.item.setAttribute('aria-expanded', String(expanded.has(r.path)));
    else r.item.removeAttribute('aria-expanded');
    const failed = r.writeError ? `\nLast write failed: ${r.writeError}` : '';
    r.row.title = `${r.path}\n${info.full}${info.reason ? '\n' + info.reason : ''}${failed}`;
    r.item.classList.toggle('readonly', !info.writable && !info.error);
    r.item.classList.toggle('error', !!info.error);
    r.item.classList.toggle('writable', info.writable);
    r.item.classList.toggle('settling', r.settling);
    r.item.classList.toggle('failed', !!r.writeError);
    const state = info.error ? 'unavailable' : info.writable ? 'editable' : 'read-only';
    r.item.setAttribute('aria-label', `${r.path}, ${info.text}, ${state}${r.writeError ? ', last write failed' : ''}`);
    r.detail.textContent = [info.error || info.reason, r.writeError && `Last write failed: ${r.writeError}`, r.expansionError].filter(Boolean).join('\n');
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
    if (event.key === 'ArrowRight') {
      if (r.info.expandable && !expanded.has(r.path)) toggleExpanded(r);
      else if (r.info.expandable) move(r, 1);
    }
    else if (event.key === 'ArrowLeft') {
      if (r.info.expandable && expanded.has(r.path)) toggleExpanded(r);
      else if (r.info.parent) rows.get(r.info.parent)?.item.focus();
    }
    else if (event.key === 'ArrowDown') move(r, 1);
    else if (event.key === 'ArrowUp') move(r, -1);
    else if (event.key === 'Home') move(r, -Infinity);
    else if (event.key === 'End') move(r, Infinity);
    else if (event.key === 'Enter' && r.info.expandable) toggleExpanded(r);
    else if (event.key === 'Enter' || event.key === 'F2') startEdit(r);
    else if (event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) openMenu(r);
    else if (event.key === 'Escape' && r.showDetail) toggleDetail(r);
    else if ((event.key === 'Delete' || (event.key === 'Backspace' && event.metaKey)) && r.info.watch) send('remove', r.info.watch);
    else if (mod && event.key.toLowerCase() === 'c') send('copy', r.path, 'value');
    else handled = false;
    if (handled) event.preventDefault();
  }

  /** @param {Row} r */
  function toggleExpanded(r) {
    if (!r.info.expandable) return;
    if (r.expansionError) r.showDetail = false;
    r.expansionError = '';
    if (expanded.has(r.path)) expanded.delete(r.path); else expanded.add(r.path);
    if (r.info.indirect) {
      const open = expanded.has(r.path);
      if (open) requested.add(r.path); else requested.delete(r.path);
      send('expand', r.path, String(open));
    }
    host.setState?.({ expanded: [...expanded].slice(0, 1000) });
    render(r);
    updateVisibility();
    r.item.focus();
  }

  function updateVisibility() {
    const visible = new Set();
    for (const path of mounted) {
      const r = rows.get(path);
      if (!r) continue;
      const parent = r.info.parent;
      r.item.hidden = !!parent && (!visible.has(parent) || !expanded.has(parent));
      if (!r.item.hidden) visible.add(path);
      else {
        if (r.input && !r.pending) closeEditor(r, false);
        if (menu?.row === r) closeMenu(false);
      }
    }
    order = [...visible];
    updateTabStops();
  }

  /** @param {Row} r */
  function toggleDetail(r) { r.showDetail = !r.showDetail; render(r); }

  /** @param {Row} r */
  function menuItems(r) {
    const { info } = r;
    /** @type {{ label: string, key?: string, run: () => void }[][]} */
    const groups = [[], [], []];
    if (info.lockable) groups[0].push({ label: info.userLocked ? 'Unlock Value' : 'Lock Value', run: () => send('lock', r.path, String(!info.userLocked)) });
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
    if (active === undefined || !order.includes(active)) active = order[0];
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
    const visit = (value, depth = 0, parent = undefined) => {
      if (!value || typeof value.path !== 'string' || next.has(value.path) || depth > 8) return;
      const error = typeof value.error === 'string' ? value.error : '';
      const children = !error && Array.isArray(value.children) ? value.children : [];
      const note = typeof value.childrenNote === 'string' ? value.childrenNote : '';
      next.set(value.path, {
        text: shortValue(value),
        full: String(value.display ?? '') + (note ? '\n' + note : ''),
        reason: typeof value.reason === 'string' && value.reason !== error ? value.reason : '',
        error,
        writable: value.writable === true && !error,
        suggest: value.writable !== true && value.userLocked !== true && !error && !children.length,
        lockable: value.lockable === true, userLocked: value.userLocked === true,
        number: typeof value.number === 'number' ? value.number : undefined,
        watch: watchOf(value.path, watches),
        depth, parent, label: typeof value.label === 'string' ? value.label : undefined,
        expandable: children.length > 0 || (value.expandable === true && !error),
        indirect: value.expandable === true,
      });
      if (value.expandable === true && expanded.has(value.path) && !requested.has(value.path)) {
        requested.add(value.path); send('expand', value.path, 'true');
      }
      for (const child of children) visit(child, depth + 1, value.path);
    };
    for (const value of m.values) visit(value);
    for (const entry of m.errors) {
      if (!entry || typeof entry.path !== 'string' || !entry.error || next.has(entry.path)) continue;
      next.set(entry.path, { text: entry.error, full: '', reason: '', error: entry.error, writable: false, suggest: false, lockable: false, userLocked: false, number: undefined, watch: entry.path, depth: 0, parent: undefined, label: undefined, expandable: false, indirect: false });
    }
    for (const [path, r] of rows) {
      if (next.has(path)) continue;
      if (menu?.row === r) closeMenu(false);
      r.item.remove();
      rows.delete(path);
    }
    const paths = [...next.keys()];
    const reorder = paths.length !== mounted.length || paths.some((p, i) => p !== mounted[i]);
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
    mounted = paths;
    empty.hidden = mounted.length > 0;
    updateVisibility();
  }

  // Suggestions use the existing watch field; focus stays in the input.
  const suggestions = el('div', 'suggestions');
  suggestions.id = 'watch-suggestions'; suggestions.hidden = true;
  suggestions.setAttribute('role', 'listbox');
  document.body.append(suggestions);
  addInput.setAttribute('role', 'combobox');
  addInput.setAttribute('aria-autocomplete', 'list');
  addInput.setAttribute('aria-controls', suggestions.id);
  addInput.setAttribute('aria-expanded', 'false');
  /** @type {{ path: string, label: string, type: string }[]} */
  let options = [];
  let chosen = -1;
  let completionId = 0;
  let chooseOnReply = false;
  let completionPending = false;
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let completionTimer;

  function hideSuggestions() {
    if (completionTimer !== undefined) clearTimeout(completionTimer);
    completionTimer = undefined;
    completionId++; completionPending = false;
    suggestions.hidden = true; options = []; chosen = -1;
    addInput.setAttribute('aria-expanded', 'false');
    addInput.removeAttribute('aria-activedescendant');
  }
  function requestSuggestions() {
    if (completionTimer !== undefined) clearTimeout(completionTimer);
    completionTimer = undefined;
    if (document.activeElement !== addInput || addInput.value.length > 1024) return;
    completionPending = true;
    host.postMessage({ type: 'complete', query: addInput.value.trim(), id: ++completionId });
  }
  function selectSuggestion(index) {
    chosen = index;
    for (let i = 0; i < suggestions.children.length; i++) {
      const item = suggestions.children[i];
      item.setAttribute('aria-selected', String(i === chosen));
      if (i === chosen) { addInput.setAttribute('aria-activedescendant', item.id); item.scrollIntoView({ block: 'nearest' }); }
    }
  }
  function acceptSuggestion() {
    if (chosen < 0 || !options[chosen]) return false;
    addInput.value = options[chosen].path;
    hideSuggestions();
    addInput.focus();
    return true;
  }
  function showSuggestions(reply) {
    if (reply.id !== completionId || reply.query !== addInput.value.trim() || document.activeElement !== addInput) return;
    completionPending = false;
    options = Array.isArray(reply.items) ? reply.items.slice(0, 50).filter(item => item && typeof item.path === 'string' && typeof item.type === 'string') : [];
    chosen = -1;
    suggestions.replaceChildren();
    for (let i = 0; i < options.length; i++) {
      const option = options[i];
      const item = el('div', 'suggestion');
      item.id = `suggestion-${i}`; item.title = `${option.path}\n${option.type}`;
      item.setAttribute('role', 'option'); item.setAttribute('aria-selected', 'false');
      item.append(el('span', 'suggestion-path', option.path), el('span', 'suggestion-type', option.type));
      item.addEventListener('mousedown', event => event.preventDefault());
      item.addEventListener('click', () => { chosen = i; acceptSuggestion(); });
      suggestions.append(item);
    }
    if (!options.length) {
      const message = el('div', 'suggestion-empty', typeof reply.message === 'string' ? reply.message : 'No matches');
      message.title = message.textContent; suggestions.append(message);
    }
    const rect = addInput.getBoundingClientRect();
    const height = Math.min(200, Math.max(24, options.length * 24));
    suggestions.style.left = `${rect.left}px`;
    suggestions.style.width = `${rect.width}px`;
    suggestions.style.maxHeight = `${Math.max(24, Math.min(200, Math.max(window.innerHeight - rect.bottom, rect.top) - 4))}px`;
    suggestions.style.top = `${window.innerHeight - rect.bottom >= height ? rect.bottom + 2 : Math.max(2, rect.top - height - 2)}px`;
    suggestions.hidden = false;
    addInput.setAttribute('aria-expanded', 'true');
    addInput.removeAttribute('aria-activedescendant');
    if (chooseOnReply && options.length) selectSuggestion(0);
    chooseOnReply = false;
    say(`${options.length} watch suggestions`);
  }

  window.addEventListener('message', event => {
    const m = event.data;
    if (!m || typeof m !== 'object') return;
    if (m.type === 'addFocus') { if (document.activeElement === addInput) requestSuggestions(); else addInput.focus(); return; }
    if (m.type === 'completions') { showSuggestions(m); return; }
    if (m.type === 'expansionReply') {
      const r = rows.get(m.path);
      if (r && m.ok === false) {
        expanded.delete(r.path); requested.delete(r.path);
        host.setState?.({ expanded: [...expanded].slice(0, 1000) });
        r.expansionError = typeof m.text === 'string' ? m.text : 'Expansion failed';
        r.showDetail = true; render(r); updateVisibility();
      }
      return;
    }
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
    if (acceptSuggestion()) return;
    const location = addInput.value.trim();
    if (location) { send('add', location); addInput.value = ''; hideSuggestions(); }
  });
  addInput.addEventListener('input', () => {
    hideSuggestions(); chooseOnReply = false;
    completionTimer = setTimeout(requestSuggestions, 120);
  });
  addInput.addEventListener('focus', requestSuggestions);
  addInput.addEventListener('blur', hideSuggestions);
  addInput.addEventListener('keydown', event => {
    const key = /** @type {KeyboardEvent} */ (event);
    if (key.isComposing) return;
    if (key.key === 'ArrowDown' || key.key === 'ArrowUp') {
      event.preventDefault();
      if (suggestions.hidden) { chooseOnReply = true; requestSuggestions(); }
      else if (options.length) selectSuggestion((chosen + (key.key === 'ArrowDown' ? 1 : chosen < 0 ? 0 : -1) + options.length) % options.length);
    } else if ((key.key === 'Enter' || key.key === 'Tab') && chosen >= 0 && acceptSuggestion()) {
      event.preventDefault();
    } else if (key.key === 'Escape') {
      event.preventDefault();
      if (!suggestions.hidden || completionTimer !== undefined || completionPending) hideSuggestions();
      else { addInput.value = ''; addInput.blur(); }
    }
  });
  window.addEventListener('blur', hideSuggestions);
  window.addEventListener('resize', hideSuggestions);
  document.addEventListener('scroll', event => { if (!suggestions.contains(/** @type {Node} */ (event.target))) hideSuggestions(); }, true);
  host.postMessage({ type: 'ready' });
})();
