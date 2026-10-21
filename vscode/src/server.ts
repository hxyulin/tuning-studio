// A studio-server child process and its stdio protocol (spec in crates/studio-server/src/protocol.rs).
// No `vscode` import, so the smoke script can drive a server from plain Node.

import { ChildProcess, spawn } from "node:child_process";

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

export class ServerError extends Error {}

export class StudioServer {
  readonly ready: Promise<Ready>;
  private readonly child: ChildProcess;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private nextId = 1;
  private input: Buffer = Buffer.alloc(0);
  private closing = false;
  private exited = false;
  private readyWaiter!: { resolve: (r: Ready) => void; reject: (e: Error) => void };

  private readonly handlers: ServerHandlers;

  constructor(path: string, args: string[], handlers: ServerHandlers) {
    this.handlers = handlers;
    this.ready = new Promise((resolve, reject) => (this.readyWaiter = { resolve, reject }));
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

  /** Call a method (the Tauri command names); rejects with the server's error message */
  call<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (this.exited) return Promise.reject(new ServerError("studio-server is not running"));
    const id = this.nextId++;
    const body = Buffer.from(JSON.stringify({ id, method, params }), "utf8");
    const header = Buffer.alloc(5);
    header.writeUInt32LE(body.length + 1, 0);
    header[4] = KIND_JSON;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.child.stdin!.write(Buffer.concat([header, body]));
    });
  }

  /** Close stdin, so the server stops its session and lets go of the probe, then exits */
  dispose(): Promise<void> {
    if (this.exited) return Promise.resolve();
    this.closing = true;
    this.child.stdin!.end();
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.child.kill();
        resolve();
      }, EXIT_GRACE_MS);
      this.child.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
    });
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
      const call = this.pending.get(m.id);
      if (!call) return;
      this.pending.delete(m.id);
      if (m.ok) call.resolve(m.result);
      else call.reject(new ServerError(m.error));
    } else if (m.type === "event") {
      this.handlers.event(m.session, m.event);
    } else if (m.type === "app_event") {
      this.handlers.appEvent?.(m.event);
    } else if (m.type === "ready") {
      this.readyWaiter.resolve({ version: m.version, mock: m.mock });
    }
  }

  private finish(code: number | null, error: string | null) {
    if (this.exited) return;
    this.exited = true;
    const why = new ServerError(error ?? `studio-server exited (code ${code})`);
    this.readyWaiter.reject(why);
    for (const call of this.pending.values()) call.reject(why);
    this.pending.clear();
    if (error) this.handlers.log(`studio-server: ${error}`);
    this.handlers.exit(code, this.closing);
  }
}
