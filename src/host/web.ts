import type { ConnectRequest, SessionEvent } from "../live/api";
import type { AppEvent } from "../live/recording";
import { appEventHub, fixedStatus, localStorageBacked, STANDALONE_STATUS } from "./common";
import type { Host, HostStartup, SessionHandlers } from "./types";

export function createWebHost(): Host {
  const token = location.hash.slice(1);
  const endpoint = (path: string) => `${path}?token=${encodeURIComponent(token)}`;
  const socket = new WebSocket(`${location.origin.replace("http", "ws")}${endpoint("/ws")}`);
  socket.binaryType = "arraybuffer";
  const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  let nextCall = 1;
  let nextSession = 1;
  let session: { id: number; handlers: SessionHandlers } | null = null;
  const appEvents = appEventHub();
  const status = structuredClone(STANDALONE_STATUS);
  status.features.reveal = { available: false, reason: "Files are saved by the local server" };
  const ready = new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("Could not reach the local Tuning Studio server"));
  });
  socket.onclose = () => {
    const error = new Error("Local server disconnected. Close other Studio tabs, then reload this page.");
    pending.forEach(p => p.reject(error));
    pending.clear();
    session?.handlers.event({ type: "status", state: "disconnected", message: error.message });
  };
  socket.onmessage = (event: MessageEvent<ArrayBuffer>) => {
    const bytes = new Uint8Array(event.data);
    if (bytes[4] === 70) {
      const id = new DataView(event.data).getUint32(5, true);
      if (session?.id === id) session.handlers.frame(event.data.slice(13));
      return;
    }
    const message = JSON.parse(new TextDecoder().decode(bytes.subarray(5)));
    if (message.type === "response") {
      const request = pending.get(message.id);
      if (!request) return;
      pending.delete(message.id);
      if (message.ok) request.resolve(message.result);
      else request.reject(new Error(message.error));
    } else if (message.type === "event" && message.session === session?.id) {
      session?.handlers.event(message.event as SessionEvent);
    } else if (message.type === "app_event") appEvents.emit(message.event as AppEvent);
  };
  async function call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    await ready;
    if (socket.readyState !== WebSocket.OPEN) throw new Error("Local server disconnected; reload this page");
    const id = nextCall++;
    return new Promise<T>((resolve, reject) => {
      pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async function pickFile(): Promise<string | null> {
    const input = document.createElement("input");
    input.type = "file";
    const file = await new Promise<File | null>(resolve => {
      input.onchange = () => resolve(input.files?.[0] ?? null);
      input.oncancel = () => resolve(null);
      input.click();
    });
    if (!file) return null;
    const response = await fetch(endpoint("/upload"), { method: "POST", body: file });
    if (!response.ok) throw new Error(`Could not upload file: ${response.status}`);
    return (await response.json()).path as string;
  }
  return {
    name: "web",
    storage: localStorageBacked,
    startup: () => call<HostStartup>("startup"),
    watchStatus: fixedStatus(status),
    canRequest: request => call("can_request", { request }),
    pickElf: pickFile,
    openElf: path => call("open_elf", { path }),
    symbolChildren: (node, limit) => call("symbol_children", { node, limit: limit ?? null }),
    inspectChildren: (node, offset, limit, live) => call("inspect_children", {node, offset, limit, live}),
    nodeMetadata: (node) => call("node_metadata", {node}),
    watchableLeaves: (node) => call("watchable_leaves", { node }),
    listProbes: () => call("list_probes"),
    listSerialPorts: () => call("list_serial_ports"),
    searchChips: (query) => call("search_chips", { query }),
    async connect(request: ConnectRequest, handlers) {
      const id = nextSession++;
      session = { id, handlers };
      await call("session_connect", { request, session: id });
    },
    disconnect: () => call("session_disconnect"),
    setRate: (hz) => call("session_set_rate", { hz }),
    setWatches: (watches) => call("session_set_watches", { watches }),
    requestValue: (id, value) => call("session_request", { id, value }),
    saveValues: () => call("session_save"),
    discardValues: () => call("session_discard"),
    taskTrace: () => call("session_task_trace"),
    taskStates: () => call("session_task_states"),
    readValues: (nodes) => call("session_read_values", { nodes }),
    recordingStart: path => call("recording_start", { path }),
    recordingStop: () => call("recording_stop"),
    exportCsv: (mcapPath, csvPath) => call("export_csv", { mcapPath, csvPath }),
    streamStart: (port, bindAll) => call("stream_start", { port, bindAll }),
    streamStop: () => call("stream_stop"),
    appState: () => call("app_state"),
    watchAppEvents: appEvents.watch,
    pickRecordingPath: async () => prompt("Recording path on the local server:"),
    pickCsvPath: async suggested => prompt("CSV path on the local server:", suggested),
    pickRecording: pickFile,
    reveal: async () => { throw new Error("Files are saved by the local server"); },
  };
}
