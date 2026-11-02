import { strict as assert } from 'node:assert';
import * as path from 'node:path';
import { test } from 'node:test';
import { chipFromRunner, parseCargoConfig, runnerEnvName } from '../cargoConfig';
import { parseJsonc } from '../jsonc';
import { pickLaunch, probeRsLaunches, substituteVariables } from '../launchConfig';
import { PlanInputs, PlanSettings, resolveRunPlan } from '../runPlan';

test('JSONC: comments and trailing commas are dropped, strings kept', () => {
  const text = `{
    // a comment
    "url": "http://example.com/*not a comment*/", /* block */
    "list": [1, 2, /* x */ 3,],
    "s": "a \\" // still string",
  }`;
  assert.deepEqual(parseJsonc(text), {
    url: 'http://example.com/*not a comment*/',
    list: [1, 2, 3],
    s: 'a " // still string',
  });
});

test('JSONC: Fleet tasks.json with comments inside arrays parses', () => {
  const text = `{ "problemMatcher": [
        // not useful here
      ], "group": { "kind": "build" // "isDefault": true,
      } }`;
  assert.deepEqual(parseJsonc(text), { problemMatcher: [], group: { kind: 'build' } });
});

test('Cargo config: build target and probe-rs runner from the training template', () => {
  const config = parseCargoConfig(`[build]
target = "thumbv7em-none-eabihf"

[target.thumbv7em-none-eabihf]
runner = "probe-rs run --chip STM32H723VGTx"
rustflags = ["-C", "target-cpu=cortex-m7", "-C", "link-arg=-Tlink.x"]
`);
  assert.deepEqual(config.buildTargets, ['thumbv7em-none-eabihf']);
  const runner = config.runners.get('thumbv7em-none-eabihf');
  assert.deepEqual(runner, ['probe-rs', 'run', '--chip', 'STM32H723VGTx']);
  assert.equal(chipFromRunner(runner!), 'STM32H723VGTx');
});

test('Cargo config: dotted keys, array runners, comments and cfg tables', () => {
  const config = parseCargoConfig(`build.target = ['thumbv7em-none-eabihf'] # comment
[target.'cfg(all(target_arch = "arm", target_os = "none"))']
runner = "ignored"
[target."thumbv8m.main-none-eabihf"]
runner = [
  "tuning-studio", # the CLI
  "run",
  "--chip=nRF5340_xxAA",
]
`);
  assert.deepEqual(config.buildTargets, ['thumbv7em-none-eabihf']);
  assert.deepEqual(config.runners.get('thumbv8m.main-none-eabihf'), [
    'tuning-studio',
    'run',
    '--chip=nRF5340_xxAA',
  ]);
  assert.equal(chipFromRunner(config.runners.get('thumbv8m.main-none-eabihf')!), 'nRF5340_xxAA');
  assert.equal(config.runners.size, 2);
});

test('Cargo runner variable name follows Cargo’s rules', () => {
  assert.equal(runnerEnvName('thumbv7em-none-eabihf'), 'CARGO_TARGET_THUMBV7EM_NONE_EABIHF_RUNNER');
  assert.equal(runnerEnvName('thumbv8m.main-none-eabihf'), 'CARGO_TARGET_THUMBV8M_MAIN_NONE_EABIHF_RUNNER');
});

const FLEET_LAUNCH = `{
  "configurations": [
    { "type": "probe-rs-debug", "request": "launch", "name": "Launch bringup",
      "chip": "STM32H723VGTx",
      "coreConfigs": [{ "coreIndex": 0,
        "programBinary": "\${workspaceFolder}/target/firmware/dm-mc02/thumbv7em-none-eabihf/release/dm-mc02-bringup" }] },
    { "type": "probe-rs-debug", "request": "attach", "name": "Attach bringup", "chip": "STM32H723VGTx",
      "coreConfigs": [{ "programBinary": "x" }], "preLaunchTask": "xtask: build" },
    { "type": "lldb", "request": "launch", "name": "Host" }
  ],
  "version": "0.2.0"
}`;

test('launch.json: probe-rs configurations are imported, others skipped', () => {
  const launches = probeRsLaunches(parseJsonc(FLEET_LAUNCH));
  assert.equal(launches.length, 2);
  assert.equal(launches[0].chip, 'STM32H723VGTx');
  assert.match(launches[0].programBinary!, /dm-mc02-bringup$/);
  assert.equal(launches[0].preLaunchTask, undefined);
  assert.equal(launches[1].preLaunchTask, 'xtask: build');
  assert.equal(pickLaunch(launches, '').launch?.name, 'Launch bringup');
  assert.equal(pickLaunch(launches, 'Attach bringup').launch?.name, 'Attach bringup');
  assert.match(pickLaunch(launches, 'Missing').error!, /no probe-rs-debug configuration named "Missing"/);
  assert.deepEqual(probeRsLaunches({ configurations: 'nope' }), []);
  assert.deepEqual(probeRsLaunches(null), []);
});

test('launch.json variables: known ones expand, unknown ones are an error', () => {
  const context = { workspaceFolder: '/w/fleet', env: { OUT: '/o' } };
  assert.equal(substituteVariables('${workspaceFolder}/a', context).value, '/w/fleet/a');
  assert.equal(substituteVariables('${env:OUT}/${workspaceFolderBasename}', context).value, '/o/fleet');
  assert.match(substituteVariables('${command:pick}', context).error!, /\$\{command:pick\} is not supported/);
});

const SETTINGS: PlanSettings = {
  executable: 'tuning-studio',
  probe: '',
  runMode: 'auto',
  launchConfiguration: '',
  chip: '',
  elf: '',
  buildTask: '',
  cargoTarget: '',
  cargoArgs: [],
};

function inputs(overrides: Omit<Partial<PlanInputs>, 'settings'> & { settings?: Partial<PlanSettings> }): PlanInputs {
  return {
    workspaceFolder: '/w',
    env: {},
    launches: [],
    hasCargoToml: true,
    ...overrides,
    settings: { ...SETTINGS, ...overrides.settings },
  };
}

test('run plan: simple Cargo project overrides the probe-rs runner', () => {
  const plan = resolveRunPlan(
    inputs({
      cargo: parseCargoConfig(
        '[build]\ntarget = "thumbv7em-none-eabihf"\n[target.thumbv7em-none-eabihf]\nrunner = "probe-rs run --chip STM32H723VGTx"\n',
      ),
      settings: { probe: '0483:3748', cargoArgs: ['--release'] },
    }),
  );
  assert.equal(plan.mode, 'cargo');
  assert.deepEqual(plan.problems, []);
  assert.deepEqual(plan.chip, { value: 'STM32H723VGTx', source: 'Cargo runner' });
  assert.deepEqual(plan.command, {
    kind: 'cargo',
    args: ['run', '--target', 'thumbv7em-none-eabihf', '--release'],
    env: {
      CARGO_TARGET_THUMBV7EM_NONE_EABIHF_RUNNER:
        'tuning-studio run --chip STM32H723VGTx --probe 0483:3748',
    },
    triple: 'thumbv7em-none-eabihf',
  });
});

test('run plan: Fleet launch.json gives task mode with the settings build task', () => {
  const plan = resolveRunPlan(
    inputs({
      workspaceFolder: '/w/fleet',
      launches: probeRsLaunches(parseJsonc(FLEET_LAUNCH)),
      settings: { buildTask: 'xtask: build' },
    }),
  );
  assert.equal(plan.mode, 'task');
  assert.deepEqual(plan.problems, []);
  assert.equal(plan.chip?.source, 'launch.json "Launch bringup"');
  assert.equal(plan.buildTask?.source, 'settings');
  assert.deepEqual(plan.command, {
    kind: 'task',
    buildTask: 'xtask: build',
    executable: 'tuning-studio',
    args: [
      'run',
      '--chip',
      'STM32H723VGTx',
      '/w/fleet/target/firmware/dm-mc02/thumbv7em-none-eabihf/release/dm-mc02-bringup',
    ],
  });
});

test('run plan: preLaunchTask is imported and settings override launch.json', () => {
  const plan = resolveRunPlan(
    inputs({
      launches: probeRsLaunches(parseJsonc(FLEET_LAUNCH)),
      settings: { launchConfiguration: 'Attach bringup', chip: 'STM32H743ZITx', elf: 'build/app.elf' },
    }),
  );
  assert.equal(plan.buildTask?.value, 'xtask: build');
  assert.deepEqual(plan.chip, { value: 'STM32H743ZITx', source: 'settings' });
  assert.equal(plan.elf?.value, path.resolve('/w', 'build/app.elf'));
  assert.equal(plan.command?.kind, 'task');
});

test('run plan: missing build task is a note, missing chip is a problem', () => {
  const noTask = resolveRunPlan(
    inputs({ launches: probeRsLaunches(parseJsonc(FLEET_LAUNCH)) }),
  );
  assert.ok(noTask.notes.some((n) => n.startsWith('No build task')));
  assert.equal(noTask.command?.kind, 'task');

  const noChip = resolveRunPlan(inputs({ settings: { elf: 'app.elf' } }));
  assert.equal(noChip.mode, 'task');
  assert.ok(noChip.problems.some((p) => p.startsWith('No chip')));
  assert.equal(noChip.command, undefined);
});

test('run plan: nothing configured, several triples, and whitespace in the runner', () => {
  const nothing = resolveRunPlan(inputs({ hasCargoToml: false }));
  assert.equal(nothing.mode, undefined);
  assert.ok(nothing.problems.some((p) => p.startsWith('No way to run')));

  const several = resolveRunPlan(
    inputs({
      settings: { runMode: 'cargo', chip: 'X' },
      cargo: { buildTargets: ['a', 'b'], runners: new Map() },
    }),
  );
  assert.ok(several.problems.some((p) => p.includes('several triples')));

  const spaced = resolveRunPlan(
    inputs({
      settings: { chip: 'X', cargoTarget: 'thumbv6m-none-eabi', executable: '/opt/my tools/tuning-studio' },
    }),
  );
  assert.equal(spaced.mode, 'cargo');
  assert.ok(spaced.problems.some((p) => p.includes('whitespace')));
  assert.equal(spaced.command, undefined);
});

test('run plan: an unknown launch configuration name stops Run', () => {
  const plan = resolveRunPlan(
    inputs({
      launches: probeRsLaunches(parseJsonc(FLEET_LAUNCH)),
      settings: { launchConfiguration: 'Gone' },
    }),
  );
  assert.equal(plan.command, undefined);
  assert.ok(plan.problems.some((p) => p.includes('"Gone"')));
});

test('run plan: the selected target is built and conflicting cargo arguments stop Run', () => {
  const settings = { chip: 'X', cargoTarget: 'thumbv6m-none-eabi' };
  const plan = resolveRunPlan(inputs({ settings }));
  assert.equal(plan.command?.kind, 'cargo');
  if (plan.command?.kind === 'cargo') {
    assert.deepEqual(plan.command.args, ['run', '--target', settings.cargoTarget]);
    assert.ok(plan.command.env.CARGO_TARGET_THUMBV6M_NONE_EABI_RUNNER);
  }
  const conflict = resolveRunPlan(inputs({ settings: { ...settings, cargoArgs: ['--target=other'] } }));
  assert.equal(conflict.command, undefined);
  assert.ok(conflict.problems.some((p) => p.includes('instead of passing --target')));
});
