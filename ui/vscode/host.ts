// The VS Code page talks only to the extension. The shared hooks import `../host`; this build
// resolves that to here, so the Tauri, browser and mock hosts stay out of the bundle.
import { createVsCodeHost } from "../../src/host/vscode";

document.documentElement.dataset.host = "vscode";

export const host = createVsCodeHost();
