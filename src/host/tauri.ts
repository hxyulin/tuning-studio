import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import * as elf from "../elf/api";
import * as live from "../live/api";
import * as rec from "../live/recording";
import { Effect } from "effect";
import { STANDALONE_STATUS, fixedStatus, localStorageBacked } from "./common";
import { HostError, runCall } from "./rpc";
import type { Host } from "./types";

/** A Tauri command that fails with `HostError`; `invoke` rejects with the command's error text */
function command<A extends unknown[], T>(method: string, run: (...args: A) => Promise<T>) {
  return (...args: A): Promise<T> =>
    runCall(Effect.tryPromise({ try: () => run(...args), catch: (e) => new HostError({ method, message: String(e) }) }));
}

/** Typed-array views need the frame to start at an 8-byte boundary */
function toAlignedBuffer(message: ArrayBuffer | Uint8Array | number[]): ArrayBuffer {
  if (message instanceof ArrayBuffer) return message;
  if (message instanceof Uint8Array) {
    return message.buffer.slice(message.byteOffset, message.byteOffset + message.byteLength) as ArrayBuffer;
  }
  return new Uint8Array(message).buffer;
}

/** The desktop app: Tauri commands, and channels for the session's streams. */
export const tauriHost: Host = {
  name: "tauri",
  canRequest: command("can_request", (request) => invoke("can_request", { request })),
  storage: localStorageBacked,
  async startup() {
    return { elfPath: await command("startup_elf_path", elf.startupElfPath)(), connect: null, connectDefaults: null, watches: [] };
  },
  watchStatus: fixedStatus(STANDALONE_STATUS),
  async pickElf() {
    const path = await open({ title: "Open firmware ELF", multiple: false, directory: false });
    return typeof path === "string" ? path : null;
  },
  openElf: command("open_elf", elf.openElf),
  symbolChildren: command("symbol_children", elf.symbolChildren),
  inspectChildren: command("inspect_children", elf.inspectChildren),
  nodeMetadata: command("node_metadata", elf.nodeMetadata),
  watchableLeaves: command("watchable_leaves", live.watchableLeaves),
  listProbes: command("list_probes", live.listProbes),
  listSerialPorts: command("list_serial_ports", live.listSerialPorts),
  searchChips: command("search_chips", live.searchChips),
  connect(request, handlers) {
    const data = new Channel<ArrayBuffer | Uint8Array | number[]>((frame) => handlers.frame(toAlignedBuffer(frame)));
    const events = new Channel<live.SessionEvent>(handlers.event);
    return command("session_connect", live.connect)(request, data as Channel<ArrayBuffer>, events);
  },
  disconnect: command("session_disconnect", live.disconnect),
  setRate: command("session_set_rate", live.setRate),
  setWatches: command("session_set_watches", live.setWatches),
  requestValue: command("session_request", live.requestValue),
  saveValues: command("session_save", live.saveValues),
  discardValues: command("session_discard", live.discardValues),
  taskStates: command("session_task_states", live.taskStates),
  taskTrace: command("session_task_trace", live.taskTrace),
  readValues: command("session_read_values", live.readValues),
  recordingStart: command("recording_start", rec.recordingStart),
  recordingStop: command("recording_stop", rec.recordingStop),
  exportCsv: command("export_csv", rec.exportCsv),
  streamStart: command("stream_start", rec.streamStart),
  streamStop: command("stream_stop", rec.streamStop),
  appState: command("app_state", rec.appState),
  watchAppEvents(listener) {
    let stop: (() => void) | null = null;
    let gone = false;
    listen<rec.AppEvent>(rec.APP_EVENT, (e) => listener(e.payload)).then((unlisten) => {
      if (gone) unlisten();
      else stop = unlisten;
    });
    return () => {
      gone = true;
      stop?.();
    };
  },
  async pickRecordingPath() {
    const path = await save({ title: "Record to", filters: [{ name: "MCAP recording", extensions: ["mcap"] }] });
    return path ?? null;
  },
  async pickCsvPath(suggested) {
    const path = await save({
      title: "Export CSV",
      defaultPath: suggested,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    return path ?? null;
  },
  async pickRecording() {
    const path = await open({
      title: "Export a recording as CSV",
      multiple: false,
      directory: false,
      filters: [{ name: "MCAP recording", extensions: ["mcap"] }],
    });
    return typeof path === "string" ? path : null;
  },
  reveal: (path) => revealItemInDir(path),
};
