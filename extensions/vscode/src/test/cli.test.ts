import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import {
  classifyFailure,
  errorMessage,
  inspectArgs,
  parseProbeList,
  setArgs,
  statusArgs,
  tasksArgs,
} from '../cliText';
import { matchLocations, parseInspect, singleLine } from '../inspect';
import { parseWebviewMessage } from '../messages';
import { Series } from '../series';
import { parseTasksOutput } from '../tasksOutput';

test('arguments never carry --chip or --elf, and pass --probe when set', () => {
  assert.deepEqual(inspectArgs(''), ['inspect']);
  assert.deepEqual(inspectArgs(' 0483:3748 ', ['a.b', '0x2000:f32']), [
    'inspect',
    '--probe',
    '0483:3748',
    '--',
    'a.b',
    '0x2000:f32',
  ]);
  assert.deepEqual(setArgs('', 'tune::KP', '-0.5'), ['set', '--', 'tune::KP', '-0.5']);
  assert.deepEqual(tasksArgs('1:2', 200), ['tasks', '--probe', '1:2', '--duration', '200']);
  assert.deepEqual(statusArgs(''), ['status']);
  for (const args of [inspectArgs('p', ['x']), setArgs('p', 'x', '1'), tasksArgs('p', 1), statusArgs('p')]) {
    assert.ok(!args.includes('--chip') && !args.includes('--elf'));
  }
});

test('failures: no session, several sessions, missing CLI, timeout', () => {
  const none = classifyFailure({
    stderr: 'Error: --chip is needed when no `run` or `attach` is running\n',
    code: 1,
    executable: 'tuning-studio',
  });
  assert.equal(none.kind, 'no-session');

  const several = classifyFailure({
    stderr:
      'Error: sessions are running for several probes (0483:3748:ABC, 1366:0105); pick one with --probe\n',
    code: 1,
    executable: 'tuning-studio',
  });
  assert.equal(several.kind, 'several-sessions');
  assert.deepEqual(several.probes, ['0483:3748:ABC', '1366:0105']);
  assert.match(several.message, /pick one with --probe/);

  assert.equal(classifyFailure({ stderr: '', code: 'ENOENT', executable: 'ts' }).kind, 'not-found');
  assert.equal(classifyFailure({ stderr: '', code: null, killed: true, executable: 'ts' }).kind, 'timeout');
});

test('error text: anyhow causes are joined, clap errors keep their first line', () => {
  assert.equal(
    errorMessage('Error: could not parse a.elf\n\nCaused by:\n    0: no such file\n    1: os error 2\n'),
    'could not parse a.elf: no such file: os error 2',
  );
  assert.equal(
    errorMessage("error: unrecognized subcommand 'inspect'\n\nUsage: tuning-studio <COMMAND>\n"),
    'This tuning-studio has no `inspect` command; install a newer CLI',
  );
  assert.equal(errorMessage("error: unexpected argument '--x' found\n\nUsage: ..."), "unexpected argument '--x' found");
});

test('probe list parses selectors with and without serials', () => {
  assert.deepEqual(
    parseProbeList('0483:3748:066DFF  STLink V2\nef1a:74e5  CMSIS-DAP\nno debug probes found\n'),
    [
      { selector: '0483:3748:066DFF', identifier: 'STLink V2' },
      { selector: 'ef1a:74e5', identifier: 'CMSIS-DAP' },
    ],
  );
  assert.deepEqual(parseProbeList('no debug probes found\n'), []);
});

const INSPECT = JSON.stringify({
  version: 1,
  values: [
    { path: 'tune_lab::KP', type: 'f32', kind: 'param', writable: true, display: '0.02', number: 0.02 },
    { path: 'motor.state', type: 'MotorSnapshot', kind: 'watch', writable: false, display: 'MotorSnapshot\n  speed: 91.1' },
    { path: 'gone', type: 'u32', kind: 'raw', writable: true, display: '', error: 'read failed' },
  ],
  tasks: [],
});

test('inspect output is validated and keeps optional fields', () => {
  const result = parseInspect(INSPECT);
  assert.equal(result.values.length, 3);
  assert.equal(result.values[0].number, 0.02);
  assert.equal(result.values[1].number, undefined);
  assert.equal(result.values[2].error, 'read failed');
});

test('inspect output with a wrong shape is rejected with a reason', () => {
  assert.throws(() => parseInspect('not json'), /did not print JSON/);
  assert.throws(() => parseInspect('{"version":2,"values":[]}'), /version 2 is not supported/);
  assert.throws(() => parseInspect('{"version":1}'), /no values array/);
  assert.throws(
    () => parseInspect('{"version":1,"values":[{"path":"a","type":"u8","kind":"magic","writable":false,"display":"1"}]}'),
    /unknown kind magic/,
  );
  assert.throws(
    () => parseInspect('{"version":1,"values":[{"path":"a","type":"u8","kind":"raw","writable":false,"display":"1","number":"1"}]}'),
    /non-numeric number/,
  );
  // A null number is the same as none.
  assert.equal(
    parseInspect('{"version":1,"values":[{"path":"a","type":"u8","kind":"raw","writable":false,"display":"1","number":null}]}')
      .values[0].number,
    undefined,
  );
});

test('manual locations match by path prefix, or by position as a fallback', () => {
  const v = (path: string) => ({ path, type: 'f32', kind: 'raw' as const, writable: true, display: '1' });
  const byPath = matchLocations(['CONTROL.yaw', 'buf'], [v('CONTROL.yaw.kp'), v('CONTROL.yaw.ki'), v('buf[0]'), v('CONTROL.yawn')]);
  assert.deepEqual(byPath.get('CONTROL.yaw')!.map((x) => x.path), ['CONTROL.yaw.kp', 'CONTROL.yaw.ki']);
  assert.deepEqual(byPath.get('buf')!.map((x) => x.path), ['buf[0]']);

  const byPosition = matchLocations(['0x2000:f32', 'KP'], [v('0x00002000'), v('tune::KP')]);
  assert.equal(byPosition.get('0x2000:f32')![0].path, '0x00002000');
  assert.equal(byPosition.get('KP')![0].path, 'tune::KP');

  const missing = matchLocations(['a', 'b'], [v('a')]);
  assert.deepEqual(missing.get('b'), []);
});

test('display text is folded to one line for tree rows', () => {
  assert.equal(singleLine('Snapshot\n  speed: 1\n  ticks: 2'), 'Snapshot speed: 1 ticks: 2');
  assert.equal(singleLine('x'.repeat(10), 5), 'xxxx…');
});

test('tasks output from the CLI README parses into tasks, states and totals', () => {
  const report = parseTasksOutput(`main: not spawned
control: running 26%, run-queued 1%
   99.1%  Suspend0 at src/main.rs:138
    0.9%  Suspend1 at src/main.rs:141
waves: running 44%, run-queued 75%, spawned in 50% of samples
  100.0%  Suspend0
outside tasks: waker::wake 20%, Executor::run 6%, select_lab::settings 3%
1105 samples in 2.00 s, 552 per second
`);
  assert.deepEqual(report.tasks.map((t) => [t.name, t.spawned, t.running, t.runQueued]), [
    ['main', false, undefined, undefined],
    ['control', true, '26%', '1%'],
    ['waves', true, '44%', '75%'],
  ]);
  assert.deepEqual(report.tasks[1].states[0], { percent: 99.1, label: 'Suspend0', file: 'src/main.rs', line: 138 });
  assert.deepEqual(report.tasks[2].states[0], { percent: 100, label: 'Suspend0', file: undefined, line: undefined });
  assert.deepEqual(report.outside, ['waker::wake 20%', 'Executor::run 6%', 'select_lab::settings 3%']);
  assert.equal(report.samples, 1105);
  assert.equal(report.seconds, 2);
  assert.deepEqual(report.other, []);
});

test('tasks output without PC sampling keeps the note as another line', () => {
  const report = parseTasksOutput(`control: run-queued 3%
  50.0%  Suspend0 at src/main.rs:10
the core has no PC sampling, so running time is not shown
1 sample in 0.20 s, 5 per second
`);
  assert.equal(report.tasks[0].running, undefined);
  assert.equal(report.tasks[0].runQueued, '3%');
  assert.deepEqual(report.other, ['the core has no PC sampling, so running time is not shown']);
  assert.equal(report.samples, 1);
});

test('series is bounded, drops the oldest, and stores gaps as null', () => {
  const s = new Series(3);
  s.push(0, 1);
  s.push(1, undefined);
  s.push(2, NaN);
  s.push(3, 4);
  assert.equal(s.length, 3);
  assert.deepEqual(s.snapshot(), { t: [1, 2, 3], v: [null, null, 4] });
  s.clear();
  assert.deepEqual(s.snapshot(), { t: [], v: [] });
  assert.throws(() => new Series(0));
});

test('webview messages: only known shapes pass', () => {
  assert.deepEqual(parseWebviewMessage({ type: 'ready' }), { type: 'ready' });
  assert.deepEqual(parseWebviewMessage({ type: 'unplot', path: 'a.b' }), { type: 'unplot', path: 'a.b' });
  assert.equal(parseWebviewMessage({ type: 'unplot', path: 'a', extra: 1 }), undefined);
  assert.equal(parseWebviewMessage({ type: 'unplot', path: 42 }), undefined);
  assert.equal(parseWebviewMessage({ type: 'unplot', path: '' }), undefined);
  assert.equal(parseWebviewMessage({ type: 'run', command: 'rm' }), undefined);
  assert.equal(parseWebviewMessage('ready'), undefined);
  assert.equal(parseWebviewMessage(null), undefined);
  assert.equal(parseWebviewMessage([{ type: 'ready' }]), undefined);
});

test('edit restrictions survive inspect parsing and layout messages reject unsupported operations', () => {
  const value = parseInspect(JSON.stringify({ version: 1, values: [{ path: 'x', type: 'u64', kind: 'raw', writable: false, display: '1', write_reason: 'task-local writes require a 1-, 2-, or 4-byte scalar' }] })).values[0];
  assert.match(value.writeReason ?? '', /task-local writes/);
  assert.equal(parseWebviewMessage({ type: 'layout', action: 'execute', id: 'a', path: '', text: '' }), undefined);
  assert.equal(parseWebviewMessage({ type: 'layout', action: 'add', id: 'a', path: '', text: '', extra: true }), undefined);
  assert.equal(parseWebviewMessage({ type: 'layout', action: 'seriesAdd', id: 'a', path: 'speed', text: '' })?.type, 'layout');
});
