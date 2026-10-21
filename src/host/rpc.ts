// Calls to the backend as Effects with typed failures. Shared by the webview hosts and the VS Code
// extension; each bundles its own copy, and errors cross the webview boundary as `{ kind, error }`.

import { Data, Deferred, Effect } from "effect";

/** The backend answered the call with an error */
export class HostError extends Data.TaggedError("HostError")<{ readonly method: string; readonly message: string }> {
  override toString() {
    return this.message;
  }
}

/** The backend went away before answering: studio-server exited, or the page lost its link */
export class Disconnected extends Data.TaggedError("Disconnected")<{ readonly message: string }> {
  override toString() {
    return this.message;
  }
}

/** No answer within `afterMs`; the backend may still finish the work */
export class TimedOut extends Data.TaggedError("TimedOut")<{
  readonly method: string;
  readonly afterMs: number;
  readonly message: string;
}> {
  override toString() {
    return this.message;
  }
}

export type CallError = HostError | Disconnected | TimedOut;

/**
 * A backstop for a backend that stops answering. studio-server bounds its own hardware requests
 * much more tightly, so a healthy call never gets near this.
 */
export const CALL_TIMEOUT_MS = 60_000;

/** Calls that wait on the person (dialogs, commands) or on the size of a file are not limited */
const UNLIMITED = new Set([
  "pick_elf",
  "pick_recording",
  "pick_recording_path",
  "pick_csv_path",
  "workbench_command",
  "export_csv",
]);

/** Rebuild a failure that crossed the webview boundary as plain data */
export function callError(method: string, kind: string | undefined, message: string): CallError {
  switch (kind) {
    case "Disconnected":
      return new Disconnected({ message });
    case "TimedOut":
      return new TimedOut({ method, afterMs: CALL_TIMEOUT_MS, message });
    default:
      return new HostError({ method, message });
  }
}

/** The tag of a failure, to send across the webview boundary */
export function errorKind(error: unknown): CallError["_tag"] {
  if (error instanceof Disconnected || error instanceof TimedOut) return error._tag;
  return "HostError";
}

interface Waiting {
  method: string;
  answer: Deferred.Deferred<unknown, CallError>;
}

/** Calls sent over one link and awaiting their answers, by id */
export class PendingCalls {
  private readonly waiting = new Map<number, Waiting>();
  private nextId = 1;
  private gone: Disconnected | null = null;
  private readonly timeoutMs: number;

  constructor(timeoutMs = CALL_TIMEOUT_MS) {
    this.timeoutMs = timeoutMs;
  }

  /** Send the call with `send(id)`; the answer comes through `succeed` or `fail` with that id */
  call<T>(method: string, send: (id: number) => void): Effect.Effect<T, CallError> {
    return Effect.suspend(() => {
      if (this.gone) return Effect.fail(this.gone);
      const id = this.nextId++;
      const answer = Deferred.makeUnsafe<unknown, CallError>();
      this.waiting.set(id, { method, answer });
      try {
        send(id);
      } catch (e) {
        this.waiting.delete(id);
        return Effect.fail(new Disconnected({ message: `Could not send ${method}: ${e}` }));
      }
      const awaited = Deferred.await(answer) as Effect.Effect<T, CallError>;
      const limited = UNLIMITED.has(method)
        ? awaited
        : Effect.timeoutOrElse(awaited, {
            duration: this.timeoutMs,
            orElse: () =>
              Effect.fail(
                new TimedOut({
                  method,
                  afterMs: this.timeoutMs,
                  message: `${method} got no answer within ${this.timeoutMs / 1000} s`,
                }),
              ),
          });
      return limited.pipe(Effect.ensuring(Effect.sync(() => this.waiting.delete(id))));
    });
  }

  succeed(id: number, result: unknown) {
    const call = this.take(id);
    if (call) Deferred.doneUnsafe(call.answer, Effect.succeed(result));
  }

  fail(id: number, message: string, kind?: string) {
    const call = this.take(id);
    if (call) Deferred.doneUnsafe(call.answer, Effect.fail(callError(call.method, kind, message)));
  }

  /** Fail every waiting call, and every later one, because the link is gone */
  disconnect(message: string) {
    this.gone = new Disconnected({ message });
    for (const call of this.waiting.values()) Deferred.doneUnsafe(call.answer, Effect.fail(this.gone));
    this.waiting.clear();
  }

  private take(id: number): Waiting | undefined {
    const call = this.waiting.get(id);
    this.waiting.delete(id);
    return call;
  }
}

/** Run a call for a Promise-based caller (React, VS Code callbacks); it rejects with the typed error */
export const runCall = <T>(effect: Effect.Effect<T, CallError>): Promise<T> => Effect.runPromise(effect);
