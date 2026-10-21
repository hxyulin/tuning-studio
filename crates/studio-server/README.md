# tuning-studio-server

The Tuning Studio backend over framed stdin/stdout, used by the VS Code extension.
It provides the same inspection, acquisition, tuning, recording and streaming
operations as the desktop app without requiring Tauri or a webview.

The package is `tuning-studio-server`; the executable is **`studio-server`**.

## Install and run

For a published release:

```sh
cargo install tuning-studio-server --locked
studio-server --version
```

Or build from the [repository](https://github.com/hxyulin/tuning-tools):

```sh
cargo build -p tuning-studio-server
```

Set VS Code's `tuningStudio.serverPath` to the installed executable if the
extension cannot find it. See the
[extension setup](https://github.com/hxyulin/tuning-tools/blob/main/docs/usage.md#vs-code-extension)
for building the extension itself. This server is normally launched by its client;
it waits for framed requests and is not an interactive shell.

| Option | Effect |
|---|---|
| `--version` | Print the package version and exit |
| `--mock` | Use an in-memory probe target initialized from the opened ELF |
| `--read-only` | Refuse target-memory writes and disable RTT consumption |

`--mock --read-only` is useful for frontend development without hardware. A mock
image does not execute firmware or simulate peripheral behavior. Normal hardware
connections require USB permissions/drivers and exclusive ownership of the probe.
Linux source builds need libudev development headers.

## Local browser

Build the frontend first, then run the standalone server (no Tauri or Node at runtime):

```sh
npm ci && npm run build
cargo build --release -p tuning-studio-server
./target/release/studio-server --web --elf /path/to/firmware.elf --chip STM32H723VGTx
```

This serves the embedded UI at an available `127.0.0.1` port and opens the default browser.
`--elf` and `--chip` populate startup and connect automatically. Without them, open an ELF in
the UI and choose connection settings, or connect to a firmware USB serial server without an ELF.
A browser file picker uploads a copy into a temporary directory removed when the server exits.
Recordings and CSV exports are written by the local server; optional custom paths refer to its
filesystem. Native file-manager reveal is unavailable in browser mode.

Attach is the default: it never deliberately halts or resets the running core. Use the matching
unstripped ELF. `--reset` explicitly resets and runs core 0 before serving; `--flash` programs and
verifies the ELF, then resets and runs it. Both require `--elf` and `--chip` and are rejected with
`--mock` or `--read-only`. They run once at process startup, never on a browser reload.

| Web option | Effect |
|---|---|
| `--elf PATH` | Initial firmware ELF |
| `--chip NAME` | Initial target chip |
| `--probe VID:PID[:SERIAL]` | Select one probe |
| `--speed-khz KHZ` | SWD clock, default 4000 kHz |
| `--port PORT` | Loopback port, default 0 (choose an available port) |
| `--no-open` | Print the URL without opening a browser |
| `--reset` | Reset existing firmware and run before serving |
| `--flash` | Program, verify, reset and run before serving |

The printed URL includes a per-launch capability in its fragment. HTTP uploads and WebSocket
connections require that capability and the exact local Origin. One browser connection owns the
backend at a time. Close other Studio tabs before reloading if a session is busy. Closing a tab
stops acquisition/recording and releases the probe; the server remains available. Ctrl-C stops
serving and releases any active session. Preserve the full printed URL to reconnect.

Control requests use JSON over WebSocket; output reuses the framed stdio binary format below,
including batched TTS1 sample frames. Queues stay bounded and a stalled writer times out. The
normal stdio protocol is unchanged. The browser service is loopback-only; it has no LAN mode.

Release assets include `studio-server-VERSION-TARGET.tar.zst` with embedded frontend assets,
README and license, plus its `.sha256`. The 0.1.2 candidate must be published before consumers
can fetch it. There has been no hardware verification of flash/reset in this change; mock tests
cover HTTP, authentication, ELF loading and repeated session connections.

## Client protocol

Each message is `u32 little-endian length | u8 kind | payload`; length includes
kind and payload but excludes the four-byte length field.

- `J`: UTF-8 JSON. Requests have `id`, `method` and `params`. Responses carry
  the same `id`, `ok`, and either `result` or `error`.
- `F`: server-to-client samples, with `u32 session | u32 reserved | TTS1 bytes`.
- The server first sends a JSON `ready` message. Session events include a session
  identifier; `app_event` messages report recording and TCP stream state.

Read stdout as binary frames and reserve stderr for diagnostics. Keep reading
frames while waiting for responses; samples and asynchronous events can arrive
between them. This protocol evolves with the extension: use matching versions.
It is separate from the firmware byte protocol and the user-facing TCP JSON stream.

The complete framing contract is in
[`protocol.rs`](https://github.com/hxyulin/tuning-tools/blob/main/crates/studio-server/src/protocol.rs);
the TypeScript client is
[`server.ts`](https://github.com/hxyulin/tuning-tools/blob/main/vscode/src/server.ts).
For scripting live data, the
[TCP stream](https://github.com/hxyulin/tuning-tools/blob/main/docs/stream.md)
is usually the simpler interface.

## Development

```sh
cargo test -p tuning-studio-server
```

Integration tests spawn the real server with a mock target and exercise framing,
connections, inspection, recording and streaming through stdin/stdout.

Part of [Tuning Studio](https://github.com/hxyulin/tuning-tools). MIT licensed.

Release 0.1.2: see the [release notes](https://github.com/hxyulin/tuning-tools/blob/main/docs/release-notes/0.1.2.md).
