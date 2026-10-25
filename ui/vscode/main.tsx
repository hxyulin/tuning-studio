import { render } from "preact";
import { useCallback, useEffect, useState } from "preact/hooks";
import type { CatalogEntry, OpenedElf, SymbolNode } from "../../src/elf/api";
import type { HostNotice } from "../../src/host/types";
import type { ConnectRequest } from "../../src/live/api";
import { formatElapsed, type RecordingState } from "../../src/live/recording";
import { useSession } from "../../src/live/useSession";
import { useWatches } from "../../src/live/useWatches";
import { host } from "./host";
import { LiveWatch } from "./LiveWatch";
import { Plot } from "./Plot";
import "./style.css";

const WINDOWS = [2, 5, 10, 30];
const SIDE_KEY = "side";

function savedSide() {
  const stored = Number(host.storage.get(SIDE_KEY));
  return Number.isFinite(stored) && stored >= 200 ? stored : 360;
}

function fileName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function workbench(command: string, onError: (e: string) => void) {
  void host.workbenchCommand?.(command).catch((e) => onError(String(e)));
}

function App() {
  const [elf, setElf] = useState<OpenedElf | null>(null);
  const [request, setRequest] = useState<ConnectRequest | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<HostNotice | null>(null);
  const [dismissed, setDismissed] = useState(0);
  const [recording, setRecording] = useState<RecordingState | null>(null);
  const [paused, setPaused] = useState(false);
  const [span, setSpan] = useState(5);
  const [revision, setRevision] = useState(0);
  const [side, setSide] = useState(savedSide);
  const session = useSession();
  const watch = useWatches(elf?.summary.path ?? null, elf);
  const connected = session.link.state === "connected";
  const live = connected && session.link.carrier === "probe";
  const catalog = session.catalog ?? elf?.catalog ?? null;

  // The extension opens firmware and connects through native commands, then asks the page to refresh
  useEffect(() => host.watchStartup?.(() => setRevision((n) => n + 1)), []);
  const { connect } = session;
  useEffect(() => {
    let stale = false;
    host
      .startup()
      .then(async (s) => {
        if (stale) return;
        const opened = s.elfPath ? await host.openElf(s.elfPath) : null;
        if (stale) return;
        setElf(opened);
        if (opened && s.connect) {
          setRequest(s.connect);
          void connect(s.connect);
        }
      })
      .catch((e) => !stale && setError(String(e)));
    return () => {
      stale = true;
    };
  }, [connect, revision]);

  useEffect(() => host.watchStatus((s) => setNotice(s.notice)), []);
  useEffect(() => {
    void host.appState().then((s) => setRecording(s.recording?.active ? s.recording : null), () => {});
    return host.watchAppEvents((e) => {
      if (e.type === "recording") setRecording(e.active ? e : null);
    });
  }, []);

  const { add, addCell, remove } = watch;
  const plot = useCallback(
    (node: SymbolNode, entry?: CatalogEntry) => {
      if (entry) addCell(entry);
      else if (node.kind === "scalar" || node.kind === "enum") add([node]);
      else
        host.watchableLeaves(node.ref).then(
          (leaves) => (leaves.length ? add(leaves) : setError(`${node.path} holds no numbers that can be plotted.`)),
          (e) => setError(`Could not plot ${node.path}: ${e}`),
        );
    },
    [add, addCell],
  );
  // Plot Symbol in the native Symbols view
  useEffect(() => {
    if (!watch.ready) return;
    return host.watchRequests?.((nodes) => nodes.forEach((n) => plot(n)));
  }, [watch.ready, plot]);

  const record = () => {
    const call = recording ? host.recordingStop() : host.recordingStart(null);
    call.then((s) => setRecording(s.active ? s : null), (e) => setError(`Recording: ${e}`));
  };

  // Saved as it settles: a drag that leaves the webview never sends its pointerup here
  useEffect(() => {
    const timer = setTimeout(() => host.storage.set(SIDE_KEY, String(Math.round(side))), 300);
    return () => clearTimeout(timer);
  }, [side]);

  const resize = (e: PointerEvent) => {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const start = e.clientX;
    const from = side;
    const end = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("lostpointercapture", end);
    };
    const move = (m: PointerEvent) => {
      if (m.buttons === 0) return end();
      setSide(Math.max(200, Math.min(from + m.clientX - start, innerWidth - 240)));
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("lostpointercapture", end);
  };

  const state = session.link.state;
  const target = request ? (request.carrier === "serial" ? request.port ?? "USB" : request.chip) : null;
  const status = connected ? target : state === "connecting" ? "Connecting…" : state === "failed" ? "Connection failed" : "Disconnected";
  const problem = error ?? (state === "failed" ? session.link.message : null);

  return (
    <div class="app" style={{ "--side": `${side}px` }}>
      <header class="bar">
        {elf ? (
          <span class="firmware" title={elf.summary.path}>
            {fileName(elf.summary.path)}
          </span>
        ) : (
          <button class="primary" onClick={() => workbench("tuningStudio.openElf", setError)}>
            Open firmware…
          </button>
        )}
        <span class="target">
          <span class={`dot ${connected ? "on" : state === "failed" ? "bad" : ""}`} />
          {status}
        </span>
        <span class="grow" />
        <span class="segmented" role="radiogroup" aria-label="Time window">
          {WINDOWS.map((w) => (
            <button role="radio" aria-checked={w === span} onClick={() => setSpan(w)}>
              {w} s
            </button>
          ))}
        </span>
        <button onClick={() => setPaused((p) => !p)} aria-pressed={paused}>
          {paused ? "Resume" : "Pause"}
        </button>
        {recording ? (
          <button class="recording" onClick={record} title={recording.path}>
            <span class="rec" />
            {formatElapsed(recording.elapsed)}
          </button>
        ) : (
          <button onClick={record} disabled={!connected}>
            Record
          </button>
        )}
        <button
          class={connected ? "" : "primary"}
          disabled={!elf && !connected}
          onClick={() => workbench(connected ? "tuningStudio.disconnect" : "tuningStudio.connect", setError)}
        >
          {connected ? "Disconnect" : "Connect…"}
        </button>
      </header>
      {notice && notice.id !== dismissed && (
        <div class="banner">
          <span class="grow">{notice.message}</span>
          {notice.reconnect && <button onClick={() => workbench("tuningStudio.connect", setError)}>Connect…</button>}
          <button class="ghost" aria-label="Dismiss" onClick={() => setDismissed(notice.id)}>
            ×
          </button>
        </div>
      )}
      {problem && (
        <div class="banner danger" role="alert">
          <span class="grow">{problem}</span>
          {error && (
            <button class="ghost" aria-label="Dismiss" onClick={() => setError(null)}>
              ×
            </button>
          )}
        </div>
      )}
      {elf ? (
        <main class="split">
          <LiveWatch
            roots={elf.roots}
            tasks={elf.tasks}
            catalog={catalog}
            tune={session.tune}
            live={live}
            watches={watch.watches}
            onPlot={plot}
            onRemove={remove}
          />
          <div class="divider" role="separator" aria-orientation="vertical" aria-label="Resize Live Watch" onPointerDown={resize} />
          <Plot watches={watch.watches} span={span} paused={paused} />
        </main>
      ) : (
        <main class="empty">Open a firmware ELF to browse and plot its variables.</main>
      )}
    </div>
  );
}

render(<App />, document.getElementById("root")!);
