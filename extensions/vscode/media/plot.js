// @ts-check
// Plot webview: draws the samples the extension sends. It builds DOM with
// textContent only and accepts only the message shapes below.
(function () {
  'use strict';

  // @ts-ignore acquireVsCodeApi is provided by the webview host.
  const vscode = acquireVsCodeApi();

  const GUTTER_LEFT = 10;
  const GUTTER_RIGHT = 56;
  const COLOR_VARS = [
    '--vscode-charts-blue',
    '--vscode-charts-orange',
    '--vscode-charts-green',
    '--vscode-charts-purple',
    '--vscode-charts-red',
    '--vscode-charts-yellow',
    '--vscode-terminal-ansiCyan',
    '--vscode-terminal-ansiMagenta',
  ];

  const lanesEl = /** @type {HTMLElement} */ (document.getElementById('lanes'));
  const statusEl = /** @type {HTMLElement} */ (document.getElementById('status'));


  /**
   * @typedef {{ path: string, t: number[], v: (number|null)[], color: string,
   *   lane: HTMLElement, canvas: HTMLCanvasElement, valueEl: HTMLElement, hidden: boolean }} Lane
   */

  const savedZoom = vscode.getState()?.zoom;
  const validZoom = value => typeof value === 'number' && Number.isFinite(value) ? Math.max(0.01, Math.min(100, value)) : 1;
  const zoom = { x: validZoom(savedZoom?.x), y: validZoom(savedZoom?.y) };
  const saveZoom = () => vscode.setState({ ...vscode.getState(), zoom });
  const state = {
    windowSeconds: 30,
    capacity: 6000,
    frozen: false,
    /** Right edge while frozen */
    frozenEnd: 0,
    latest: 0,
    /** @type {number | null} host time under the pointer */
    hover: null,
    /** @type {Lane[]} */
    lanes: [],
    theme: readTheme(),
  };

  const send = (action, id = '', path = '', text = '') => vscode.postMessage({ type: 'layout', action, id, path, text });
  let legendBox = vscode.getState()?.legendBox;
  const plotResize = new ResizeObserver(schedule);
  let frame = 0;
  function schedule() {
    if (!frame) {
      frame = requestAnimationFrame(() => {
        frame = 0;
        draw();
      });
    }
  }

  function cssVar(name, fallback) {
    const value = getComputedStyle(document.body).getPropertyValue(name).trim();
    return value || fallback;
  }

  function readTheme() {
    return {
      fg: cssVar('--vscode-foreground', '#ccc'),
      muted: cssVar('--vscode-descriptionForeground', '#999'),
      grid: cssVar('--vscode-charts-lines', 'rgba(128,128,128,0.5)'),
      font: cssVar('--vscode-editor-font-family', 'monospace'),
      colors: COLOR_VARS.map((name, i) => cssVar(name, ['#3794ff', '#d18616', '#89d185', '#b180d7', '#f14c4c', '#cca700', '#29b8db', '#bc3fbc'][i])),
    };
  }

  // ---- messages -----------------------------------------------------------

  function isNumberArray(a) {
    return Array.isArray(a) && a.every((x) => typeof x === 'number' && Number.isFinite(x));
  }

  function isValueArray(a) {
    return Array.isArray(a) && a.every((x) => x === null || (typeof x === 'number' && Number.isFinite(x)));
  }

  window.addEventListener('message', (event) => {
    const m = event.data;
    if (typeof m !== 'object' || m === null) {
      return;
    }
    switch (m.type) {
      case 'init':
        if (
          typeof m.windowSeconds === 'number' &&
          typeof m.capacity === 'number' &&
          typeof m.frozen === 'boolean' &&
          typeof m.frozenEnd === 'number' && Number.isFinite(m.frozenEnd) &&
          Array.isArray(m.plots) && Array.isArray(m.choices) &&
          Array.isArray(m.series) &&
          m.series.every(
            (s) =>
              typeof s === 'object' && s !== null && typeof s.path === 'string' &&
              isNumberArray(s.t) && isValueArray(s.v) && s.t.length === s.v.length,
          )
        ) {
          init(m);
        }
        break;
      case 'append':
        if (
          typeof m.t === 'number' && Number.isFinite(m.t) && Array.isArray(m.values) &&
          m.values.every(
            (p) => Array.isArray(p) && p.length === 2 && typeof p[0] === 'string' &&
              (p[1] === null || (typeof p[1] === 'number' && Number.isFinite(p[1]))),
          )
        ) {
          append(m.t, m.values);
        }
        break;
      case 'view':
        if (typeof m.windowSeconds === 'number' && typeof m.frozen === 'boolean' &&
          typeof m.frozenEnd === 'number' && Number.isFinite(m.frozenEnd)) {
          setView(m.windowSeconds, m.frozen, m.frozenEnd);
        }
        break;
      case 'status':
        if (typeof m.text === 'string') {
          statusEl.textContent = m.text;
        }
        break;
    }
  });

  function init(m) {
    state.windowSeconds = Math.max(1, m.windowSeconds);
    state.capacity = Math.max(1, Math.floor(m.capacity));
    plotResize.disconnect();
    lanesEl.replaceChildren();
    state.lanes = [];
    const plot = m.plots[0] || { id: '', title: 'Plot', paths: [], hidden: [] };
    const card = document.createElement('section'); card.className = 'plot-card';
    const head = document.createElement('header');
    const title = document.createElement('button'); title.className = 'plot-title'; title.textContent = plot.title;
    title.title = 'Rename plot'; title.setAttribute('aria-label', `Rename ${plot.title}`);
    title.addEventListener('click', () => {
      const input = document.createElement('input'); input.className = 'title-editor'; input.value = plot.title;
      input.maxLength = 80; input.setAttribute('aria-label', 'Plot title');
      let done = false;
      const finish = (save) => {
        if (done) return;
        done = true;
        if (save && input.value.trim()) {
          title.textContent = input.value.trim();
          send('rename', plot.id, '', input.value);
        }
        input.replaceWith(title); title.focus();
      };
      input.addEventListener('keydown', event => {
        if (event.key === 'Enter') { event.preventDefault(); finish(true); }
        if (event.key === 'Escape') { event.preventDefault(); finish(false); }
      });
      input.addEventListener('blur', () => finish(false));
      title.replaceWith(input); input.focus(); input.select();
    });
    head.append(title);
    if (m.plots.length) {
      const close = document.createElement('button'); close.className = 'close-plot'; close.textContent = '×';
      close.title = 'Remove plot'; close.setAttribute('aria-label', 'Remove plot');
      close.addEventListener('click', () => send('remove', plot.id)); head.append(close);
    }
    const body = document.createElement('div'); body.className = 'plot-body';
    const legend = document.createElement('div'); legend.className = 'legend';
    const grip = document.createElement('div'); grip.className = 'legend-grip';
    grip.textContent = 'Variables'; grip.title = 'Drag to move; resize from the bottom-right corner'; legend.append(grip);
    const placeLegend = () => {
      if (!legendBox) return;
      const width = Math.min(legendBox.width, Math.max(120, body.clientWidth));
      const height = Math.min(legendBox.height, Math.max(60, body.clientHeight));
      legend.style.width = `${width}px`; legend.style.height = `${height}px`;
      legend.style.left = `${Math.max(0, Math.min(legendBox.x, body.clientWidth - width))}px`;
      legend.style.top = `${Math.max(0, Math.min(legendBox.y, body.clientHeight - height))}px`;
    };
    const rememberLegend = () => {
      legendBox = { x: legend.offsetLeft, y: legend.offsetTop, width: legend.offsetWidth, height: legend.offsetHeight };
      vscode.setState({ ...vscode.getState(), legendBox });
    };
    grip.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      event.preventDefault(); grip.setPointerCapture(event.pointerId);
      const x = event.clientX, y = event.clientY, left = legend.offsetLeft, top = legend.offsetTop;
      const move = event => {
        legend.style.left = `${Math.max(0, Math.min(body.clientWidth - legend.offsetWidth, left + event.clientX - x))}px`;
        legend.style.top = `${Math.max(0, Math.min(body.clientHeight - legend.offsetHeight, top + event.clientY - y))}px`;
      };
      const finish = () => { grip.removeEventListener('pointermove', move); rememberLegend(); };
      grip.addEventListener('pointermove', move); grip.addEventListener('pointerup', finish, { once: true });
      grip.addEventListener('lostpointercapture', finish, { once: true });
    });
    legend.addEventListener('pointerup', rememberLegend);
    body.addEventListener('dragover', event => {
      if (!event.dataTransfer?.types.includes('application/x-tuning-studio-watch')) return;
      event.preventDefault(); event.dataTransfer.dropEffect = 'copy'; body.classList.add('drop-target');
    });
    body.addEventListener('dragleave', event => { if (!body.contains(/** @type {Node|null} */ (event.relatedTarget))) body.classList.remove('drop-target'); });
    body.addEventListener('drop', event => {
      body.classList.remove('drop-target');
      const path = event.dataTransfer?.getData('application/x-tuning-studio-watch');
      if (!path) return;
      event.preventDefault();
      if (m.choices.includes(path) && !plot.paths.includes(path) && plot.paths.length < 8) send('seriesAdd', plot.id, path);
    });
    const select = document.createElement('select'); select.setAttribute('aria-label', 'Add variable to plot');
    const placeholder = document.createElement('option'); placeholder.textContent = '+ Add variable'; placeholder.value = ''; select.append(placeholder);
    for (const path of m.choices.filter(path => !plot.paths.includes(path))) {
      const option = document.createElement('option'); option.textContent = path; option.value = path; select.append(option);
    }
    select.disabled = plot.paths.length >= 8 || select.options.length === 1;
    select.addEventListener('change', () => { if (select.value) send('seriesAdd', plot.id, select.value); });
    const chart = document.createElement('div'); chart.className = 'chart';
    const canvas = document.createElement('canvas'); canvas.className = 'plot-canvas'; canvas.setAttribute('role', 'img');
    canvas.setAttribute('aria-label', `${plot.title}: ${plot.paths.join(', ')}`);
    const axis = document.createElement('canvas'); axis.className = 'plot-axis';
    axis.tabIndex = 0; axis.setAttribute('role', 'slider'); axis.setAttribute('aria-label', 'Time scale');
    axis.setAttribute('aria-valuemin', '0.01'); axis.setAttribute('aria-valuemax', '36000');
    axis.title = 'Drag left/right to stretch time. Double-click to reset. Arrow keys adjust.';
    canvas.title = 'Scroll to stretch time; Shift+scroll to stretch values. Drag the right scale up/down. Double-click to reset both scales.';
    const adjustZoom = (dimension, value) => {
      zoom[dimension] = validZoom(value); saveZoom(); schedule();
      const seconds = Math.max(0.01, Math.min(36000, state.windowSeconds * zoom.x));
      axis.setAttribute('aria-valuenow', String(seconds));
      axis.setAttribute('aria-valuetext', `${seconds.toPrecision(3)} seconds`);
    };
    adjustZoom('x', zoom.x);
    const stretch = (event, dimension, surface) => {
      if (event.button !== 0) return;
      event.preventDefault(); surface.setPointerCapture(event.pointerId);
      const origin = dimension === 'x' ? event.clientX : event.clientY;
      const initial = zoom[dimension];
      const move = event => adjustZoom(dimension, initial * Math.exp(((dimension === 'x' ? event.clientX : event.clientY) - origin) * -1 / 120));
      const finish = () => { surface.removeEventListener('pointermove', move); };
      surface.addEventListener('pointermove', move);
      surface.addEventListener('pointerup', finish, { once: true });
      surface.addEventListener('lostpointercapture', finish, { once: true });
    };
    axis.addEventListener('pointerdown', event => stretch(event, 'x', axis));
    canvas.addEventListener('pointerdown', event => {
      if (event.clientX >= canvas.getBoundingClientRect().right - GUTTER_RIGHT) stretch(event, 'y', canvas);
    });
    axis.addEventListener('dblclick', () => adjustZoom('x', 1));
    canvas.addEventListener('dblclick', () => { adjustZoom('x', 1); adjustZoom('y', 1); });
    axis.addEventListener('keydown', event => {
      if (['ArrowLeft', 'ArrowRight', 'Home'].includes(event.key)) {
        event.preventDefault(); adjustZoom('x', event.key === 'Home' ? 1 : zoom.x * (event.key === 'ArrowRight' ? 0.8 : 1.25));
      }
    });
    chart.addEventListener('wheel', event => {
      if (event.ctrlKey || /** @type {Element} */ (event.target).closest('.legend')) return;
      event.preventDefault();
      const dimension = event.shiftKey ? 'y' : 'x';
      const delta = event.deltaY || event.deltaX;
      const pixels = delta * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? chart.clientHeight : 1);
      adjustZoom(dimension, zoom[dimension] * Math.exp(Math.max(-100, Math.min(100, pixels)) * (dimension === 'x' ? 1 : -1) / 500));
    }, { passive: false });
    chart.append(canvas, axis); body.append(legend, chart); card.append(head, body); lanesEl.append(card);
    plotResize.observe(canvas);
    requestAnimationFrame(placeLegend);
    plotResize.observe(body);
    window.onresize = placeLegend;
    for (const path of plot.paths) {
      const series = m.series.find(s => s.path === path);
      if (series) state.lanes.push(makeLane(path, series.t.slice(), series.v.slice(), state.lanes.length, legend, canvas, plot.id, !!plot.hidden?.includes(path)));
    }
    legend.append(select);
    if (!plot.paths.length) {
      const empty = document.createElement('p'); empty.className = 'empty-chart'; empty.textContent = 'Add a variable to start plotting.'; chart.append(empty);
    }
    state.latest = 0;
    for (const lane of state.lanes) {
      if (lane.t.length) {
        state.latest = Math.max(state.latest, lane.t[lane.t.length - 1]);
      }
    }
    setView(state.windowSeconds, m.frozen, m.frozenEnd);
    schedule();
  }

  function setView(windowSeconds, frozen, frozenEnd) {
    state.windowSeconds = Math.max(1, windowSeconds);
    state.frozenEnd = frozenEnd;
    state.frozen = frozen;
    schedule();
  }

  function append(t, values) {
    state.latest = Math.max(state.latest, t);
    for (const [path, value] of values) {
      const lane = state.lanes.find((l) => l.path === path);
      if (!lane) {
        continue;
      }
      lane.t.push(t);
      lane.v.push(value);
      // Trim in batches so a full buffer does not shift on every sample.
      if (lane.t.length > state.capacity * 1.1) {
        const extra = lane.t.length - state.capacity;
        lane.t.splice(0, extra);
        lane.v.splice(0, extra);
      }
    }
    if (!state.frozen) {
      schedule();
    }
  }

  /** @returns {Lane} */
  function makeLane(path, t, v, index, legend, canvas, plotId, hidden) {
    const lane = document.createElement('section');
    lane.className = 'lane';
    const head = document.createElement('button');
    head.className = 'lane-head';
    head.type = 'button';
    head.setAttribute('aria-pressed', String(!hidden));
    head.setAttribute('aria-label', `${hidden ? 'Show' : 'Hide'} ${path}`);
    head.title = `${hidden ? 'Show' : 'Hide'} ${path}`;
    lane.classList.toggle('is-hidden', hidden);
    head.addEventListener('click', () => send('seriesToggle', plotId, path));
    const swatch = document.createElement('span');
    swatch.className = 'swatch';
    const color = state.theme.colors[index % state.theme.colors.length];
    swatch.style.background = color;
    const pathEl = document.createElement('span');
    pathEl.className = 'path';
    const pathText = document.createElement('bdi'); pathText.dir = 'ltr'; pathText.textContent = path; pathEl.append(pathText);
    pathEl.title = path;
    const valueEl = document.createElement('span');
    valueEl.className = 'value';
    const unplot = document.createElement('button');
    unplot.className = 'unplot';
    unplot.type = 'button';
    unplot.textContent = '×';
    unplot.title = 'Remove from plot';
    unplot.setAttribute('aria-label', `Remove ${path} from plot`);
    unplot.addEventListener('click', () => send('seriesRemove', plotId, path));
    head.append(swatch, pathEl, valueEl);
    lane.append(head, unplot);
    legend.append(lane);
    return { path, t, v, color, lane, canvas, valueEl, hidden };
  }

  // ---- drawing ------------------------------------------------------------

  function formatValue(v) {
    if (v === 0) {
      return '0';
    }
    const abs = Math.abs(v);
    if (abs >= 1e6 || abs < 1e-4) {
      return v.toExponential(3);
    }
    return String(Number(v.toPrecision(6)));
  }

  /** Ticks at 1, 2 or 5 times a power of ten. */
  function niceTicks(min, max, count) {
    const span = max - min;
    if (!(span > 0)) {
      return [min];
    }
    const raw = span / Math.max(1, count);
    const power = Math.pow(10, Math.floor(Math.log10(raw)));
    const step = [1, 2, 5, 10].map((m) => m * power).find((s) => s >= raw) || 10 * power;
    const ticks = [];
    for (let x = Math.ceil(min / step) * step; x <= max + step * 1e-9; x += step) {
      ticks.push(Math.abs(x) < step * 1e-9 ? 0 : x);
    }
    return ticks;
  }

  /** Index of the last sample at or before t, or -1. */
  function indexAtOrBefore(times, t) {
    let lo = 0;
    let hi = times.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (times[mid] <= t) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    return found;
  }

  function fitCanvas(canvas) {
    const dpr = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const height = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const ctx = /** @type {CanvasRenderingContext2D} */ (canvas.getContext('2d'));
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, width: canvas.clientWidth, height: canvas.clientHeight };
  }

  function timeRange() {
    const end = state.frozen ? state.frozenEnd : state.latest;
    return { start: end - Math.max(0.01, Math.min(36000, state.windowSeconds * zoom.x)), end };
  }

  function draw() {
    const range = timeRange();
    const canvas = /** @type {HTMLCanvasElement|null} */ (document.querySelector('.plot-canvas'));
    if (canvas) {
      const { ctx, width, height } = fitCanvas(canvas);
      ctx.clearRect(0, 0, width, height);
      const visible = state.lanes.filter(lane => !lane.hidden);
      for (const lane of state.lanes) {
        const at = indexAtOrBefore(lane.t, state.hover ?? range.end);
        const value = at >= 0 ? lane.v[at] : null;
        lane.valueEl.textContent = value === null ? '—' : formatValue(value);
      }
      visible.forEach((lane, index) => drawLane(lane, range, index === 0));
      if (state.lanes.length && !visible.length) {
        ctx.fillStyle = state.theme.muted; ctx.textAlign = 'center'; ctx.font = `12px ${state.theme.font}`;
        ctx.fillText('All variables hidden', width / 2, height / 2);
      }
    }
    document.querySelectorAll('.plot-axis').forEach(axis => drawAxis(range, /** @type {HTMLCanvasElement} */ (axis)));
  }

  function drawLane(lane, range, clear) {
    const { ctx, width, height } = fitCanvas(lane.canvas);
    const theme = state.theme;
    if (clear) ctx.clearRect(0, 0, width, height);
    const left = GUTTER_LEFT;
    const right = width - GUTTER_RIGHT;
    const top = 4;
    const bottom = height - 6;
    if (right <= left || bottom <= top) {
      return;
    }
    const x = (t) => left + ((t - range.start) / (range.end - range.start)) * (right - left);

    // Visible samples, plus one on each side so lines reach the edges.
    const first = Math.max(0, indexAtOrBefore(lane.t, range.start));
    const lastIndex = indexAtOrBefore(lane.t, range.end);
    let min = Infinity;
    let max = -Infinity;
    for (const sibling of state.lanes.filter(s => s.canvas === lane.canvas && !s.hidden)) {
      const begin = Math.max(0, indexAtOrBefore(sibling.t, range.start));
      const end = indexAtOrBefore(sibling.t, range.end);
      for (let i = begin; i <= end; i++) {
        const v = sibling.v[i];
        if (v !== null) { min = Math.min(min, v); max = Math.max(max, v); }
      }
    }

    // Header value: under the pointer, or the newest sample.
    const at = state.hover !== null ? indexAtOrBefore(lane.t, state.hover) : lastIndex;
    const headValue = at >= 0 ? lane.v[at] : null;
    lane.valueEl.textContent = headValue === null ? (at >= 0 ? 'no value' : '') : formatValue(headValue);
    lane.valueEl.classList.toggle('gap', headValue === null);

    ctx.font = `10px ${theme.font}`;
    ctx.textBaseline = 'middle';
    if (!Number.isFinite(min)) {
      ctx.fillStyle = theme.muted;
      ctx.textAlign = 'center';
      ctx.fillText(lane.t.length ? 'no numeric samples in this window' : 'waiting for samples', (left + right) / 2, (top + bottom) / 2);
      drawHover(ctx, x, top, bottom);
      return;
    }
    if (min === max) {
      const pad = Math.abs(min) * 0.05 || 1;
      min -= pad;
      max += pad;
    } else {
      const pad = (max - min) * 0.08;
      min -= pad;
      max += pad;
    }
    const center = min / 2 + max / 2;
    const half = (max - min) / (2 * zoom.y);
    min = center - half; max = center + half;
    const y = (v) => bottom - ((v - min) / (max - min)) * (bottom - top);

    // Draw the shared scale once per plot.
    if (clear) {
    const ticks = niceTicks(min, max, Math.max(1, Math.floor((bottom - top) / 28)));
    ctx.strokeStyle = theme.grid;
    ctx.globalAlpha = 0.35;
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (const tick of ticks) {
      const py = Math.round(y(tick)) + 0.5;
      ctx.moveTo(left, py);
      ctx.lineTo(right, py);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
    ctx.fillStyle = theme.muted;
    ctx.textAlign = 'left';
    for (const tick of ticks) {
      ctx.fillText(formatValue(tick), right + 6, y(tick));
    }

    }

    // The trace, broken at every missing sample.
    ctx.save();
    ctx.beginPath();
    ctx.rect(left, top - 2, right - left, bottom - top + 4);
    ctx.clip();
    ctx.strokeStyle = lane.color;
    ctx.fillStyle = lane.color;
    ctx.lineWidth = 1.5;
    ctx.lineJoin = 'round';
    ctx.beginPath();
    let open = false;
    let runStart = -1;
    const end = Math.min(lane.t.length - 1, lastIndex + 1);
    for (let i = first; i <= end; i++) {
      const v = lane.v[i];
      if (v === null) {
        if (open && runStart === i - 1) {
          dot(ctx, x(lane.t[i - 1]), y(lane.v[i - 1]));
        }
        open = false;
        continue;
      }
      const px = x(lane.t[i]);
      const py = y(v);
      if (open) {
        ctx.lineTo(px, py);
      } else {
        ctx.moveTo(px, py);
        open = true;
        runStart = i;
      }
    }
    ctx.stroke();
    if (open && runStart === end && end >= first) {
      dot(ctx, x(lane.t[end]), y(/** @type {number} */ (lane.v[end])));
    }
    // Mark the newest value.
    if (lastIndex >= 0 && lane.v[lastIndex] !== null && state.hover === null) {
      ctx.beginPath();
      ctx.arc(x(lane.t[lastIndex]), y(/** @type {number} */ (lane.v[lastIndex])), 2.5, 0, Math.PI * 2);
      ctx.fill();
    }
    // The hovered sample.
    if (state.hover !== null && at >= 0 && lane.v[at] !== null) {
      ctx.beginPath();
      ctx.arc(x(lane.t[at]), y(/** @type {number} */ (lane.v[at])), 3, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
    drawHover(ctx, x, top, bottom);
  }

  function dot(ctx, px, py) {
    ctx.save();
    ctx.beginPath();
    ctx.arc(px, py, 1.5, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }

  function drawHover(ctx, x, top, bottom) {
    if (state.hover === null) {
      return;
    }
    const px = Math.round(x(state.hover)) + 0.5;
    ctx.strokeStyle = state.theme.fg;
    ctx.globalAlpha = 0.4;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(px, top);
    ctx.lineTo(px, bottom);
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  function drawAxis(range, axisEl) {
    const { ctx, width, height } = fitCanvas(axisEl);
    const theme = state.theme;
    ctx.clearRect(0, 0, width, height);
    const left = GUTTER_LEFT;
    const right = width - GUTTER_RIGHT;
    if (right <= left || state.lanes.length === 0) {
      return;
    }
    const span = range.end - range.start;
    const x = (t) => left + ((t - range.start) / span) * (right - left);
    ctx.font = `10px ${theme.font}`;
    ctx.textBaseline = 'middle';
    ctx.fillStyle = theme.muted;
    ctx.textAlign = 'center';
    // Ticks in seconds before the newest sample.
    const ticks = niceTicks(-span, 0, Math.max(2, Math.floor((right - left) / 80)));
    for (const tick of ticks) {
      const px = x(range.end + tick);
      if (px < left + 12) {
        continue;
      }
      ctx.fillText(tick === 0 ? '0 s' : `${formatValue(tick)} s`, px, height / 2);
    }
    if (state.hover !== null) {
      const offset = state.hover - range.end;
      const text = `${offset.toFixed(offset > -10 ? 2 : 1)} s`;
      const px = Math.min(right - 24, Math.max(left + 24, x(state.hover)));
      const w = ctx.measureText(text).width + 10;
      ctx.fillStyle = cssVar('--vscode-editorHoverWidget-background', '#252526');
      ctx.fillRect(px - w / 2, 2, w, height - 4);
      ctx.fillStyle = theme.fg;
      ctx.fillText(text, px, height / 2);
    }
    if (state.frozen) {
      ctx.textAlign = 'right';
      ctx.fillStyle = theme.muted;
      ctx.fillText('frozen', width - 6, height / 2);
    }
  }

  // ---- input --------------------------------------------------------------

  lanesEl.addEventListener('pointermove', (event) => {
    if (/** @type {Element} */ (event.target).closest('.legend')) { state.hover = null; schedule(); return; }
    const card = /** @type {Element} */ (event.target).closest('.plot-card');
    const target = card?.querySelector('canvas');
    if (!target) {
      return;
    }
    const rect = target.getBoundingClientRect();
    const left = rect.left + GUTTER_LEFT;
    const right = rect.right - GUTTER_RIGHT;
    if (event.clientX < left || event.clientX > right) {
      state.hover = null;
    } else {
      const range = timeRange();
      state.hover = range.start + ((event.clientX - left) / (right - left)) * (range.end - range.start);
    }
    schedule();
  });
  lanesEl.addEventListener('pointerleave', () => {
    state.hover = null;
    schedule();
  });

  new ResizeObserver(schedule).observe(document.body);
  // Theme changes swap the body class; read the colors again.
  new MutationObserver(() => {
    state.theme = readTheme();
    state.lanes.forEach((lane, i) => {
      lane.color = state.theme.colors[i % state.theme.colors.length];
      const swatch = /** @type {HTMLElement | null} */ (lane.lane.querySelector('.swatch'));
      if (swatch) {
        swatch.style.background = lane.color;
      }
    });
    schedule();
  }).observe(document.body, { attributes: true, attributeFilter: ['class'] });

  vscode.postMessage({ type: 'ready' });
})();
