import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

test('mutable watch suggestions insert an editable snippet at the chosen line, and refuse statements', async () => {
  const exports: Record<string, (arg?: string) => Promise<void>> = {};
  let selection = 'pid.kp';
  let inserted = '';
  let position: unknown;
  const end = { line: 12, character: 30 };
  const commands: unknown[][] = [];
  class Snippet {
    constructor(public value: string) {}
    appendText(text: string) { this.value += text; return this; }
    appendPlaceholder(text: string, index: number) { this.value += '${' + index + ':' + text + '}'; return this; }
  }
  runInNewContext(readFileSync(path.join(__dirname, '../sourceSuggestion.js'), 'utf8'), {
    exports,
    require: () => ({
      SnippetString: Snippet,
      window: { showInformationMessage: () => {}, activeTextEditor: {
        selection: { end },
        document: { languageId: 'rust', getText: () => selection, lineAt: () => ({ text: '    update(pid.kp);', range: { end } }) },
        insertSnippet: async (snippet: Snippet, at: unknown) => { inserted = snippet.value; position = at; },
      } },
      commands: { executeCommand: async (...args: unknown[]) => { commands.push(args); } },
    }),
  });
  await exports.insertMutableWatch();
  assert.match(inserted, /tuning_studio_api::watch_mut!/);
  assert.match(inserted, /"pid.kp.edit"/);
  assert.match(inserted, /&mut \$\{3:pid.kp\}/);
  assert.match(inserted, /\$\{2:f32\}/);
  assert.equal(position, end);
  selection = 'let kp = 3;'; inserted = '';
  await exports.insertMutableWatch();
  assert.equal(inserted, '', 'do not turn a selected statement into a mutable borrow');
  await exports.suggestMutableWatch('motor.speed');
  assert.equal(commands[0][0], 'workbench.action.findInFiles');
  assert.equal((commands[0][1] as { query: string }).query, 'speed');
});
