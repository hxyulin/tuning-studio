// Exercise the real webview bridge without navigating a browser or needing hardware.
import assert from "node:assert/strict";
import { build } from "esbuild";
import { runInNewContext } from "node:vm";

const { outputFiles } = await build({
  entryPoints: ["../src/host/vscode.ts"], bundle: true, write: false, platform: "node", format: "cjs",
});
let receive;
let navigations = 0;
let acquired = 0;
const calls = [];
const context = {
  exports: {}, module: { exports: {} },
  ArrayBuffer, Uint8Array,
  // Effect schedules call timeouts and fiber yields on the page's timers
  setTimeout, clearTimeout, queueMicrotask,
  document: {
    body: { classList: { contains: () => false } },
    documentElement: { dataset: {} },
    getElementById: () => null,
  },
  MutationObserver: class { observe() {} },
  window: {
    addEventListener: (_type, listener) => { receive = listener; },
    location: { reload: () => { navigations++; } },
  },
  acquireVsCodeApi: () => {
    acquired++;
    return { getState: () => undefined, setState() {}, postMessage(message) { calls.push(message); } };
  },
};
runInNewContext(outputFiles[0].text, context);
const host = context.module.exports.createVsCodeHost();
const answer = (method, result) => {
  const message = calls.findLast(m => m.method === method);
  assert.ok(message, `call to ${method}`);
  receive({ data: { type: "result", id: message.id, ok: true, result } });
};

// A refresh can arrive while another RPC is still pending. Keep that bridge alive.
const pending = host.listProbes();
let refreshes = 0;
const unsubscribe = host.watchStartup(() => { refreshes++; });
receive({ data: { type: "refresh" } });
assert.equal(navigations, 0, "refresh must not navigate to VS Code's fake.html");
assert.equal(refreshes, 1);
answer("list_probes", [{ selector: "mock", name: "mock", serial: null }]);
assert.equal((await pending)[0].selector, "mock");

const request = { carrier: "probe", chip: "STM32H723VGTx", probe: null, port: null, speedKhz: 4000, rateHz: 100 };
const startup = host.startup();
answer("startup", { elfPath: "firmware.elf", connect: request, resumeSession: 42, watches: [], connectDefaults: null });
await startup;
const events = [];
let frames = 0;
const attached = host.connect(request, { event: e => events.push(e), frame: () => { frames++; } });
answer("session_attach", [{ type: "status", state: "connected", message: null }]);
await attached;
receive({ data: { type: "frame", session: 42, data: new ArrayBuffer(8) } });
assert.equal(frames, 1);
assert.equal(events[0].state, "connected");
assert.equal(calls.filter(m => m.method === "session_connect").length, 0, "reattach without reconnecting the probe");
assert.equal(acquired, 1, "keep the original VS Code bridge");
unsubscribe();
receive({ data: { type: "refresh" } });
assert.equal(refreshes, 1);
assert.equal(navigations, 0);
console.log("webview refresh: no navigation, pending RPC retained, existing session reattached");
