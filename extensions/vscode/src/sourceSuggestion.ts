import * as vscode from 'vscode';

/** A source search is intentional: an inspection path is not a source expression. */
export async function suggestMutableWatch(path: string): Promise<void> {
  const expression = path.split(/::|\./).at(-1)?.replace(/\[.*$/, '') ?? '';
  await vscode.commands.executeCommand('workbench.action.findInFiles', {
    query: /^[A-Za-z_][A-Za-z0-9_]*$/.test(expression) ? expression : path,
    filesToInclude: '**/*.rs', isRegex: false, matchWholeWord: true,
  });
  void vscode.window.showInformationMessage('Find the variable in its task, select the mutable scalar expression, then choose “Tuning Studio: Insert watch_mut! for Selection”. The snippet needs its scalar type and should run before the task uses the value.');
}

/** Insert only at a user-chosen source position, with editable type and expression. */
export async function insertMutableWatch(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== 'rust') return;
  const selected = editor.document.getText(editor.selection).trim();
  if (!/^[A-Za-z_][A-Za-z0-9_]*(?:(?:\.[A-Za-z_][A-Za-z0-9_]*)|(?:\[\d+\]))*$/.test(selected) || selected.length > 200) {
    void vscode.window.showInformationMessage('Select a mutable scalar expression, such as pid.kp, on one line first.');
    return;
  }
  const line = editor.document.lineAt(editor.selection.end.line);
  const indent = line.text.match(/^\s*/)?.[0] ?? '';
  const snippet = new vscode.SnippetString('\n' + indent + 'tuning_studio_api::watch_mut!(');
  snippet.appendPlaceholder(JSON.stringify(selected + ".edit"), 1).appendText(': ').appendPlaceholder('f32', 2)
    .appendText(' = &mut ').appendPlaceholder(selected, 3).appendText(');');
  await editor.insertSnippet(snippet, line.range.end);
}
