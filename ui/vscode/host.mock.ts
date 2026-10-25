// `npm run dev:vscode`: the VS Code page in a browser, against the simulated firmware. The
// workbench commands stand in for the extension's native Connect and Disconnect.
import { mockHost } from "../../src/host/mock";
import type { Host } from "../../src/host/types";

document.documentElement.dataset.host = "vscode";

const refreshes = new Set<() => void>();
let connected = false;

export const host: Host = {
  ...mockHost,
  async startup() {
    const startup = await mockHost.startup();
    return { ...startup, connect: connected ? startup.connect : null, watches: [] };
  },
  watchStartup(listener) {
    refreshes.add(listener);
    return () => refreshes.delete(listener);
  },
  async workbenchCommand(command) {
    if (command === "tuningStudio.connect") connected = true;
    if (command === "tuningStudio.disconnect") {
      connected = false;
      await mockHost.disconnect();
    }
    refreshes.forEach((refresh) => refresh());
  },
};
