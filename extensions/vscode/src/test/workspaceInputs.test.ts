import * as assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import type { readInputs } from '../workspaceInputs';

function harness(options: { elf?: string; name?: string; text?: string }) {
  const exports: Record<string, unknown> = {};
  runInNewContext(readFileSync(path.join(__dirname, '../workspaceInputs.js'), 'utf8'), {
    exports, process, TextDecoder,
    require(name: string) {
      if (name === './settings') return { elf: () => options.elf ?? '', launchConfiguration: () => options.name ?? '' };
      if (name === 'vscode') return {
        Uri: { joinPath: () => ({}) },
        workspace: { fs: { readFile: async () => {
          if (options.text === undefined) throw Object.assign(new Error('missing'), { code: 'FileNotFound' });
          return Buffer.from(options.text);
        } } },
      };
      return require(name.startsWith('./') ? path.join(__dirname, '..', name) : name);
    },
  });
  return () => (exports.readInputs as typeof readInputs)({ uri: { fsPath: '/project' } } as never);
}

const launch = JSON.stringify({ configurations: [
  { type: 'probe-rs-debug', request: 'launch', name: 'first', coreConfigs: [{ programBinary: '${workspaceFolder}/first.elf' }], preLaunchTask: 'do not execute' },
  { type: 'probe-rs-debug', request: 'attach', name: 'selected', coreConfigs: [{ programBinary: 'selected.elf' }] },
] });

test('offline suggestions resolve the selected launch ELF without build configuration', async () => {
  const inputs = await harness({ name: 'selected', text: launch })();
  assert.equal(inputs.elf, '/project/selected.elf');
  assert.equal(inputs.elfSource, 'launch.json "selected"');
  assert.equal(inputs.launchError, undefined);
});

test('explicit ELF works without launch.json and overrides imported paths', async () => {
  assert.equal((await harness({ elf: 'manual.elf' })()).elf, '/project/manual.elf');
  assert.equal((await harness({ elf: '/tmp/manual.elf', text: launch })()).elf, '/tmp/manual.elf');
});

test('missing launch files are optional and invalid selection or paths explain the failure', async () => {
  const absent = await harness({})();
  assert.equal(absent.elf, undefined);
  assert.equal(absent.launchError, undefined);
  assert.match((await harness({ name: 'gone', text: launch })()).launchError!, /gone/);
  const unsupported = launch.replace('${workspaceFolder}', '${command:pick}');
  assert.match((await harness({ text: unsupported })()).launchError!, /not supported/);
  assert.ok((await harness({ text: '{ invalid' })()).launchError);
});
