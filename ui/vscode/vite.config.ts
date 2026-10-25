import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/**
 * The shared hooks in src/live import `../host`, which would pull in every host. Both they and this
 * page get the extension's host, or the simulated firmware's in `--mode mock`.
 */
const host = (file: string): Plugin => ({
  name: "vscode-host",
  enforce: "pre",
  resolveId(source, importer) {
    const shared = source === "../host" && importer?.includes("/src/live/");
    const page = source === "./host" && importer?.includes("/ui/vscode/");
    if (shared || page) return here(file);
  },
});

// `npm run build:webview`: the extension's page. Relative asset paths (the extension rewrites them
// to webview URIs) and one script, so a nonce covers all of it under the webview CSP.
export default defineConfig(({ mode }) => ({
  root: here("."),
  base: "./",
  plugins: [host(mode === "mock" ? "./host.mock.ts" : "./host.ts")],
  oxc: { jsx: { runtime: "automatic", importSource: "preact" } },
  resolve: { alias: { react: "preact/compat" } },
  build: {
    outDir: here("../../vscode/media"),
    emptyOutDir: true,
    modulePreload: false,
    rolldownOptions: { output: { codeSplitting: false } },
  },
}));
