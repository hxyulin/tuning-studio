import { useCallback, useEffect, useRef, useState } from "react";
import { host } from "../host";
import { MenuItem, Popover, button, ghostButton } from "../ui";
import { type CsvExport, type RecordingState, csvPathFor, formatBytes, formatElapsed } from "./recording";
import { useCapture } from "./useCapture";
import { useHostStatus } from "./useHostStatus";

const LAST_KEY = "lastRecording";

type Notice =
  | { kind: "recorded"; state: RecordingState }
  | { kind: "exported"; csv: CsvExport }
  | { kind: "error"; message: string };

function fileName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

function message(e: unknown) {
  return e instanceof Error ? e.message : String(e);
}

/** Recording state and actions, shared by the toolbar button and the notice under it */
export function useRecorder() {
  const { recording } = useCapture();
  const { features } = useHostStatus();
  const [notice, setNotice] = useState<Notice | null>(null);
  const [last, setLast] = useState<string | null>(() => host.storage.get(LAST_KEY));
  const wasActive = useRef(false);

  // A recording that ends, by Stop or with its session, leaves a notice
  useEffect(() => {
    if (!recording) return;
    if (recording.active) {
      wasActive.current = true;
      return;
    }
    if (!wasActive.current) return;
    wasActive.current = false;
    setNotice({ kind: "recorded", state: recording });
    setLast(recording.path);
    try {
      host.storage.set(LAST_KEY, recording.path);
    } catch {
      // Not remembered next launch
    }
  }, [recording]);

  const start = useCallback(async (choosePath: boolean) => {
    try {
      const path = choosePath ? await host.pickRecordingPath() : null;
      if (choosePath && !path) return;
      setNotice(null);
      await host.recordingStart(path);
    } catch (e) {
      setNotice({ kind: "error", message: `Could not start recording: ${message(e)}` });
    }
  }, []);

  const stop = useCallback(async () => {
    try {
      await host.recordingStop();
    } catch (e) {
      setNotice({ kind: "error", message: `Could not stop recording: ${message(e)}` });
    }
  }, []);

  /** Export `mcap`, or a recording the person picks, to a CSV they choose */
  const exportCsv = useCallback(async (mcap: string | null) => {
    try {
      const source = mcap ?? (await host.pickRecording());
      if (!source) return;
      const target = await host.pickCsvPath(csvPathFor(source));
      if (!target) return;
      setNotice({ kind: "exported", csv: await host.exportCsv(source, target) });
    } catch (e) {
      setNotice({ kind: "error", message: `Could not export CSV: ${message(e)}` });
    }
  }, []);

  const reveal = useCallback(async (path: string) => {
    try {
      await host.reveal(path);
    } catch (e) {
      setNotice({ kind: "error", message: `Could not show ${fileName(path)}: ${message(e)}` });
    }
  }, []);

  return {
    recording: recording?.active ? recording : null,
    last,
    notice,
    features,
    start,
    stop,
    exportCsv,
    reveal,
    dismiss: () => setNotice(null),
  };
}

export type Recorder = ReturnType<typeof useRecorder>;

/** Record button with its menu; elapsed time, size and Stop while recording */
export function RecordButton({ rec, connected }: { rec: Recorder; connected: boolean }) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLSpanElement>(null);
  const close = useCallback(() => setOpen(false), []);
  const { record, exportCsv } = rec.features;

  if (rec.recording) {
    const r = rec.recording;
    return (
      <span className="flex items-center gap-1.5">
        <span
          title={r.path}
          className="inline-flex items-center gap-1.5 rounded-full border border-danger bg-panel pr-2.5 pl-2 text-[12px] leading-5 tabular-nums"
        >
          <span aria-hidden className="h-[7px] w-[7px] animate-pulse rounded-full bg-danger" />
          <span className="font-medium text-danger">Rec</span>
          {formatElapsed(r.elapsed)} · {formatBytes(r.bytes)}
          {r.dropped > 0 && (
            <span className="text-warn" title="Sample batches lost because the disk fell behind">
              · {r.dropped} dropped
            </span>
          )}
        </span>
        <button onClick={() => void rec.stop()} className={`${button} text-[12px]`}>
          Stop
        </button>
      </span>
    );
  }

  const canRecord = connected && record.available;
  const why = !record.available ? (record.reason ?? undefined) : connected ? undefined : "Connect to the target to record";
  const pick = (action: () => void) => () => {
    setOpen(false);
    action();
  };
  return (
    <>
      <span ref={anchor} className="inline-flex">
        <button
          onClick={() => void rec.start(false)}
          disabled={!canRecord}
          title={why ?? "Record every sample, the log and tuning changes to an MCAP file"}
          className={`${button} inline-flex items-center gap-1.5 rounded-r-none text-[12px]`}
        >
          <span aria-hidden className="h-[8px] w-[8px] rounded-full bg-danger" />
          Record
        </button>
        <button
          onClick={() => setOpen((o) => !o)}
          aria-label="Recording options"
          aria-haspopup="menu"
          aria-expanded={open}
          className={`${button} -ml-px rounded-l-none px-1.5 text-[12px]`}
        >
          ▾
        </button>
      </span>
      <Popover anchor={anchor} open={open} onClose={close} place="below-left" label="Recording">
        <div role="menu">
          <MenuItem onSelect={pick(() => void rec.start(false))} disabled={!canRecord} title={why}>
            Record
          </MenuItem>
          <MenuItem onSelect={pick(() => void rec.start(true))} disabled={!canRecord} title={why}>
            Record to…
          </MenuItem>
          <hr className="my-1 border-rule" />
          <MenuItem
            onSelect={pick(() => void rec.exportCsv(rec.last))}
            disabled={!exportCsv.available || !rec.last}
            title={exportCsv.reason ?? rec.last ?? "Nothing recorded yet"}
          >
            Export CSV of last recording
            {rec.last && <span className="ml-1 font-mono text-faint">{fileName(rec.last)}</span>}
          </MenuItem>
          <MenuItem
            onSelect={pick(() => void rec.exportCsv(null))}
            disabled={!exportCsv.available}
            title={exportCsv.reason ?? undefined}
          >
            Export CSV of a recording…
          </MenuItem>
        </div>
      </Popover>
    </>
  );
}

/** What the last recording or export did, with its follow-up actions */
export function RecordNotice({ rec }: { rec: Recorder }) {
  const n = rec.notice;
  if (!n) return null;
  const { exportCsv, reveal } = rec.features;
  const revealButton = (path: string) => (
    <button
      onClick={() => void rec.reveal(path)}
      disabled={!reveal.available}
      title={reveal.reason ?? "Show the file in its folder"}
      className={`${button} text-[12px]`}
    >
      Reveal
    </button>
  );
  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-rule bg-panel px-3 py-1 text-[12px]"
    >
      {n.kind === "error" ? (
        <span className="min-w-0 flex-1 text-danger">{n.message}</span>
      ) : n.kind === "recorded" ? (
        <>
          <span className="min-w-0 flex-1 truncate" title={n.state.path}>
            {n.state.error ? (
              <span className="text-danger">{n.state.error}. </span>
            ) : (
              "Recorded "
            )}
            {n.state.elapsed.toFixed(1)} s, {n.state.ticks.toLocaleString()} ticks, {formatBytes(n.state.bytes)}
            {n.state.dropped > 0 && <span className="text-warn">, {n.state.dropped} batches dropped</span>} to{" "}
            <span className="font-mono">{fileName(n.state.path)}</span>
          </span>
          <button
            onClick={() => void rec.exportCsv(n.state.path)}
            disabled={!exportCsv.available}
            title={exportCsv.reason ?? "Write the samples as a CSV, one column per value"}
            className={`${button} text-[12px]`}
          >
            Export CSV
          </button>
          {revealButton(n.state.path)}
        </>
      ) : (
        <>
          <span className="min-w-0 flex-1 truncate" title={n.csv.path}>
            Wrote {n.csv.rows.toLocaleString()} rows of {n.csv.columns} {n.csv.columns === 1 ? "value" : "values"} to{" "}
            <span className="font-mono">{fileName(n.csv.path)}</span>
            {n.csv.truncated && (
              <span className="text-warn"> (the recording was cut short; read up to its last whole chunk)</span>
            )}
          </span>
          {revealButton(n.csv.path)}
        </>
      )}
      <button onClick={rec.dismiss} className={`${ghostButton} text-[12px]`}>
        Dismiss
      </button>
    </div>
  );
}
