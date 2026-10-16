import { createWebHost } from "./web";
import { mockHost } from "./mock";
import { tauriHost } from "./tauri";
import type { Host } from "./types";
import { createVsCodeHost, inVsCode } from "./vscode";

export type {
  ConnectDefaults,
  Host,
  HostNotice,
  HostSources,
  HostStartup,
  HostStatus,
  SessionHandlers,
  SourceState,
  WatchSeed,
} from "./types";

/**
 * A VS Code webview gets the extension; `?host=mock` forces the simulation, and a plain browser
 * tab gets it too, as it has no backend.
 */
function pick(): Host {
  if (inVsCode()) {
    // The CSS maps its tokens to the editor theme under this attribute
    document.documentElement.dataset.host = "vscode";
    return createVsCodeHost();
  }
  const asked = new URLSearchParams(location.search).get("host");
  if (asked === "web") return createWebHost();
  if (asked === "mock" || !("__TAURI_INTERNALS__" in window)) return mockHost;
  return tauriHost;
}

export const host: Host = pick();
