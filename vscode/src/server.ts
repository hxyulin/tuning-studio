// A studio-server child process and its stdio protocol (spec in crates/studio-server/src/protocol.rs).
// No `vscode` import, so the smoke script can drive a server from plain Node.

import { type ChildProcess, spawn } from "node:child_process";
import { Deferred, Effect } from "effect";
import { type CallError, Disconnected, PendingCalls, runCall } from "../../src/host/rpc";

const KIND_JSON = 0x4a; // 'J'
const KIND_FRAME = 0x46; // 'F'
/** How long a closed server gets to release the probe before it is killed */
const EXIT_GRACE_MS = 3000;

export interface Ready {
  version: string;
  mock: boolean;
}

export interface ServerHandlers {
  /** A session's status, stats, log, tune or catalog event */
  event(session: number, event: { type: string; [key: string]: unknown }): void;
  /** Recording progress and stream state, over every session (studio-app `AppEvent`) */
  appEvent?(event: { type: string; [key: string]: unknown }): void;
  /** A TTS1 sample frame, copied into a buffer of its own (so 8-byte aligned) */
  frame(session: number, tts1: Uint8Array): void;
  /** The process ended; `expected` when `dispose` asked it to */
  exit(code: number | null, expected: boolean): void;
  /** stderr output and protocol problems */
  log(line: string): void;
}

export class StudioServer {
  /** The server's `ready` message; fails with `Disconnected` if it exits first */
  readonly started = Deferred.makeUnsafe<Ready, Disconnected>();
  readonly ready: Promise<Ready>;
  private readonly child: ChildProcess;
  private readonly pending = new PendingCalls();
  private input: Buffer = Buffer.alloc(0);
  private closing = false;
  private exited = false;

  private readonly handlers: ServerHandlers;

  constructor(path: string, args: string[], handlers: ServerHandlers) {
    this.handlers = handlers;
    this.ready = runCall(Deferred.await(this.started));
    this.ready.catch(() => {}); // a server that never starts also shows up as `exit`
    this.child = spawn(path, args, { stdio: ["pipe", "pipe", "pipe"] });
    this.child.stdout!.on("data", (chunk: Buffer) => this.receive(chunk));
    this.child.stderr!.setEncoding("utf8");
    this.child.stderr!.on("data", (text: string) => {
      for (const line of text.split(/\r?\n/)) if (line) handlers.log(line);
    });
    this.child.stdin!.on("error", () => {}); // a dead server shows up as `exit`
    this.child.on("error", (e) => this.finish(null, e.message));
    // `close` comes after stdout is drained, so the last events are delivered first
    this.child.on("close", (code) => this.finish(code, null));
  }

  get running(): boolean {
    return !this.exited;
  }

  /** Call a method (the Tauri command names); fails with the server's error, `Disconnected` or `TimedOut` */
  callEffect<T = unknown>(method: string, params: Record<string, unknown> = {}): Effect.Effect<T, CallError> {
    return this.pending.call<T>(method, (id) => {
      const body = Buffer.from(JSON.stringify({ id, method, params }), "utf8");
      const header = Buffer.alloc(5);
      header.writeUInt32LE(body.length + 1, 0);
      header[4] = KIND_JSON;
      this.child.stdin!.write(Buffer.concat([header, body]));
    });
  }

  call<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    return runCall(this.callEffect<T>(method, params));
  }

  /** Close stdin, so the server stops its session and lets go of the probe, then exits */
  dispose(): Promise<void> {
    if (this.exited) return Promise.resolve();
    this.closing = true;
    this.child.stdin!.end();
    const closed = Effect.callback<void>((resume) => {
      const done = () => resume(Effect.void);
      this.child.once("close", done);
      return Effect.sync(() => this.child.off("close", done));
    });
    const graceful = Effect.timeoutOrElse(closed, {
      duration: EXIT_GRACE_MS,
      orElse: () => Effect.sync(() => void this.child.kill()),
    });
    return Effect.runPromise(graceful);
  }

  private receive(chunk: Buffer) {
    this.input = this.input.length ? Buffer.concat([this.input, chunk]) : chunk;
    let at = 0;
    while (this.input.length - at >= 4) {
      const len = this.input.readUInt32LE(at);
      if (this.input.length - at - 4 < len) break;
      const body = this.input.subarray(at + 4, at + 4 + len);
      at += 4 + len;
      try {
        this.dispatch(body);
      } catch (e) {
        this.handlers.log(`bad message from studio-server: ${e}`);
      }
    }
    this.input = at === this.input.length ? Buffer.alloc(0) : this.input.subarray(at);
  }

  private dispatch(body: Buffer) {
    if (body[0] === KIND_FRAME) {
      const session = body.readUInt32LE(1);
      // A fresh buffer: the webview views the columns as Float64Arrays
      const tts1 = new Uint8Array(body.length - 9);
      tts1.set(body.subarray(9));
      this.handlers.frame(session, tts1);
      return;
    }
    if (body[0] !== KIND_JSON) throw new Error(`unknown kind ${body[0]}`);
    const m = JSON.parse(body.subarray(1).toString("utf8"));
    if (m.type === "response") {
      if (m.ok) this.pending.succeed(m.id, m.result);
      else this.pending.fail(m.id, m.error);
    } else if (m.type === "event") {
      this.handlers.event(m.session, m.event);
    } else if (m.type === "app_event") {
      this.handlers.appEvent?.(m.event);
    } else if (m.type === "ready") {
      Deferred.doneUnsafe(this.started, Effect.succeed({ version: m.version, mock: m.mock }));
    }
  }

  private finish(code: number | null, error: string | null) {
    if (this.exited) return;
    this.exited = true;
    const why = error ?? `studio-server exited (code ${code})`;
    Deferred.doneUnsafe(this.started, Effect.fail(new Disconnected({ message: why })));
    this.pending.disconnect(why);
    if (error) this.handlers.log(`studio-server: ${error}`);
    this.handlers.exit(code, this.closing);
  }
}
