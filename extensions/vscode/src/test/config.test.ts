import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { parseJsonc } from '../jsonc';
import { pickLaunch, probeRsLaunches, substituteVariables } from '../launchConfig';

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
