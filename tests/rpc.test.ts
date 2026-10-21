import { test } from "node:test";
import assert from "node:assert/strict";
import { callError, Disconnected, errorKind, HostError, PendingCalls, runCall, TimedOut } from "../src/host/rpc.ts";

test("an answer settles only the call with its id", async () => {
  const pending = new PendingCalls();
  const sent: number[] = [];
  const first = runCall(pending.call<number>("list_probes", (id) => sent.push(id)));
  const second = runCall(pending.call<number>("list_probes", (id) => sent.push(id)));
  pending.succeed(sent[1], 2);
  pending.succeed(sent[0], 1);
  pending.succeed(99, 3);
  assert.deepEqual(await Promise.all([first, second]), [1, 2]);
});

test("a refused call fails with the backend's message, shown without a prefix", async () => {
  const pending = new PendingCalls();
  const call = runCall(pending.call("session_request", (id) => pending.fail(id, "demo.gain takes 0 to 10")));
  const error = await call.catch((e: unknown) => e);
  assert.ok(error instanceof HostError);
  assert.equal(error.method, "session_request");
  assert.equal(String(error), "demo.gain takes 0 to 10");
});

test("a lost link fails waiting and later calls as Disconnected", async () => {
  const pending = new PendingCalls();
  const waiting = runCall(pending.call("open_elf", () => {}));
  pending.disconnect("studio-server exited (code 1)");
  const later = runCall(pending.call("open_elf", () => assert.fail("nothing is sent once the link is gone")));
  for (const call of [waiting, later]) {
    const error = await call.catch((e: unknown) => e);
    assert.ok(error instanceof Disconnected);
    assert.equal(error.message, "studio-server exited (code 1)");
  }
});

test("an unanswered call times out, but a dialog waits for the person", async () => {
  const pending = new PendingCalls(20);
  const error = await runCall(pending.call("list_probes", () => {})).catch((e: unknown) => e);
  assert.ok(error instanceof TimedOut);
  assert.equal(error.method, "list_probes");
  let dialogId = 0;
  const dialog = runCall(pending.call<string>("pick_elf", (id) => (dialogId = id)));
  setTimeout(() => pending.succeed(dialogId, "/tmp/firmware.elf"), 60);
  assert.equal(await dialog, "/tmp/firmware.elf");
});

test("a failure keeps its kind across the webview boundary", () => {
  for (const error of [
    new HostError({ method: "m", message: "refused" }),
    new Disconnected({ message: "gone" }),
    new TimedOut({ method: "m", afterMs: 1, message: "late" }),
  ]) {
    const rebuilt = callError("m", errorKind(error), error.message);
    assert.equal(rebuilt._tag, error._tag);
    assert.equal(rebuilt.message, error.message);
  }
  assert.equal(errorKind(new Error("plain")), "HostError");
});
