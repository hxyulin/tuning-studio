/** A parsed `tuning-studio tasks` report. Lines the parser does not know are kept. */
export interface TaskState {
  percent: number;
  label: string;
  file?: string;
  line?: number;
}

export interface TaskReport {
  name: string;
  spawned: boolean;
  /** The rest of the task's line, such as `running 26%, run-queued 1%`. */
  summary: string;
  running?: string;
  runQueued?: string;
  states: TaskState[];
}

export interface TasksOutput {
  tasks: TaskReport[];
  /** `outside tasks: a 20%, b 6%`, split into its parts. */
  outside: string[];
  /** `1105 samples in 2.00 s, 552 per second` */
  samplesLine?: string;
  samples?: number;
  seconds?: number;
  /** Lines the parser did not recognize, such as a note about PC sampling. */
  other: string[];
}

export function parseTasksOutput(text: string): TasksOutput {
  const out: TasksOutput = { tasks: [], outside: [], other: [] };
  let current: TaskReport | undefined;
  for (const raw of text.split(/\r?\n/)) {
    if (raw.trim() === '') {
      continue;
    }
    const state = /^\s+([\d.]+)%\s+(.*?)(?:\s+at\s+(.+):(\d+))?\s*$/.exec(raw);
    if (state && current) {
      current.states.push({
        percent: Number(state[1]),
        label: state[2],
        file: state[3],
        line: state[4] === undefined ? undefined : Number(state[4]),
      });
      continue;
    }
    const line = raw.trim();
    const samples = /^(\d+) samples? in ([\d.]+) s\b/.exec(line);
    if (samples) {
      out.samplesLine = line;
      out.samples = Number(samples[1]);
      out.seconds = Number(samples[2]);
      current = undefined;
      continue;
    }
    if (line.startsWith('outside tasks: ')) {
      out.outside = line
        .slice('outside tasks: '.length)
        .split(', ')
        .filter((p) => p !== '');
      current = undefined;
      continue;
    }
    const task = /^(\S.*?): (not spawned|(?:running \S+, )?run-queued .*)$/.exec(line);
    if (task && !raw.startsWith(' ')) {
      const summary = task[2];
      current = {
        name: task[1],
        spawned: summary !== 'not spawned',
        summary,
        running: /running (\S+?),/.exec(summary)?.[1],
        runQueued: /run-queued (\S+?)(?:,|$)/.exec(summary)?.[1],
        states: [],
      };
      out.tasks.push(current);
      continue;
    }
    out.other.push(line);
    current = undefined;
  }
  return out;
}
