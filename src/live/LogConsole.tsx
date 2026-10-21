import { useLayoutEffect, useMemo, useRef } from "react";
import { ghostButton } from "../ui";
import type { LogLine, StreamState } from "./api";

const LEVELS = ["error", "warn", "info", "debug", "trace"];
const SHOWN = 1000;

const levelClass: Record<string, string> = {
  trace: "text-faint",
  debug: "text-faint",
  info: "text-info",
  warn: "text-warn",
  error: "text-danger",
};

export interface LogFilter {
  levels: Set<string>;
  text: string;
  /** Keep the newest line in view */
  follow: boolean;
}

export const defaultLogFilter: LogFilter = { levels: new Set(["error", "warn", "info", "debug"]), text: "", follow: true };

const pill = "rounded-full border border-rule px-[7px] text-[11px] leading-[17px] text-muted aria-pressed:bg-surface aria-pressed:text-ink";

export function LogTools({
  filter,
  onChange,
  onClear,
}: {
  filter: LogFilter;
  onChange: (filter: LogFilter) => void;
  onClear: () => void;
}) {
  return (
    <>
      <input
        value={filter.text}
        onChange={(e) => onChange({ ...filter, text: e.currentTarget.value })}
        placeholder="Filter"
        aria-label="Show lines containing"
        spellCheck={false}
        className="mr-1 w-28 min-w-0 rounded-sm border border-rule bg-surface px-1.5 font-mono text-[12px] leading-[17px] placeholder:text-faint"
      />
      {LEVELS.map((level) => (
        <button
          key={level}
          aria-pressed={filter.levels.has(level)}
          onClick={() => {
            const levels = new Set(filter.levels);
            if (!levels.delete(level)) levels.add(level);
            onChange({ ...filter, levels });
          }}
          className={pill}
        >
          {level}
        </button>
      ))}
      <button
        aria-pressed={filter.follow}
        onClick={() => onChange({ ...filter, follow: !filter.follow })}
        title="Scroll to new lines as they arrive"
        className={pill}
      >
        follow
      </button>
      <button onClick={onClear} className={ghostButton}>
        Clear
      </button>
    </>
  );
}

interface Props {
  lines: LogLine[];
  stream: StreamState | null;
  connected: boolean;
  filter: LogFilter;
  onFollowChange: (follow: boolean) => void;
}

export function LogView({ lines, stream, connected, filter, onFollowChange }: Props) {
  const list = useRef<HTMLDivElement>(null);
  const needle = filter.text.trim().toLowerCase();
  const visible = useMemo(() => {
    const out = lines.filter(
      (l) =>
        (l.level === null || filter.levels.has(l.level)) &&
        (!needle || l.message.toLowerCase().includes(needle) || l.module?.toLowerCase().includes(needle)),
    );
    return out.length > SHOWN ? out.slice(out.length - SHOWN) : out;
  }, [lines, filter.levels, needle]);

  useLayoutEffect(() => {
    const el = list.current;
    if (el && filter.follow) el.scrollTop = el.scrollHeight;
  }, [visible, filter.follow]);

  let empty = "Connect to see the firmware's defmt log.";
  if (connected) {
    if (!stream || stream.state === "absent") empty = "This firmware has no RTT log (no _SEGGER_RTT symbol or defmt table).";
    else if (stream.state === "searching") empty = "Waiting for the firmware to set up RTT…";
    else if (lines.length) empty = needle ? `No shown line contains “${filter.text.trim()}”.` : "Every line is hidden by the level filters.";
    else empty = "No log lines yet.";
  }

  return (
    <div
      ref={list}
      role="log"
      aria-live="off"
      onScroll={(e) => {
        const el = e.currentTarget;
        const atEnd = el.scrollHeight - el.scrollTop - el.clientHeight < 8;
        if (atEnd !== filter.follow) onFollowChange(atEnd);
      }}
      className="min-h-0 flex-1 overflow-auto font-mono text-[12px] select-text"
    >
      {visible.length === 0 ? (
        <p className="px-3 py-2 font-sans text-[13px] text-muted">{empty}</p>
      ) : (
        <table className="w-full border-collapse">
          <tbody>
            {visible.map((l, i) => (
              <tr key={i} title={l.location ?? undefined} className="align-top hover:bg-panel">
                <td className="px-2 py-px text-right whitespace-nowrap text-faint tabular-nums">
                  {l.timestamp ?? l.hostTime.toFixed(3)}
                </td>
                <td className={`px-2 py-px text-[11px] whitespace-nowrap uppercase ${levelClass[l.level ?? ""] ?? "text-faint"}`}>
                  {l.level ?? ""}
                </td>
                <td className="px-2 py-px whitespace-nowrap text-muted">{l.module ?? ""}</td>
                <td className={`w-full px-2 py-px break-words whitespace-pre-wrap ${l.level === "error" ? "text-danger" : ""}`}>
                  {l.message}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
