import * as vscode from 'vscode';
import { TaskReport, TaskState, TasksOutput } from './tasksOutput';

type TaskNode =
  | { type: 'task'; task: TaskReport }
  | { type: 'state'; state: TaskState; task: string }
  | { type: 'outside'; parts: string[] }
  | { type: 'part'; text: string }
  | { type: 'line'; text: string };

export interface TasksSample {
  output: TasksOutput;
  raw: string;
  at: Date;
  durationMs: number;
}

/** Tasks: the last `tuning-studio tasks` sample. Nothing here updates by itself. */
export class TasksTree implements vscode.TreeDataProvider<TaskNode> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  sample?: TasksSample;
  sampling?: number;
  error?: string;

  refresh(): void {
    this.changed.fire();
  }

  message(): string | undefined {
    if (this.sampling !== undefined) {
      return `Sampling for ${this.sampling} ms`;
    }
    const taken = this.sample
      ? `Sample from ${this.sample.at.toLocaleTimeString()}${
          this.sample.output.samplesLine ? `: ${this.sample.output.samplesLine}` : ''
        }`
      : undefined;
    if (this.error) {
      return taken ? `Sampling failed: ${this.error}. ${taken}` : `Sampling failed: ${this.error}`;
    }
    return taken;
  }

  getChildren(node?: TaskNode): TaskNode[] {
    const output = this.sample?.output;
    if (!output) {
      return [];
    }
    if (!node) {
      const nodes: TaskNode[] = output.tasks.map((task) => ({ type: 'task', task }));
      if (output.outside.length > 0) {
        nodes.push({ type: 'outside', parts: output.outside });
      }
      nodes.push(...output.other.map((text): TaskNode => ({ type: 'line', text })));
      return nodes;
    }
    if (node.type === 'task') {
      return node.task.states.map((state) => ({ type: 'state', state, task: node.task.name }));
    }
    if (node.type === 'outside') {
      return node.parts.map((text) => ({ type: 'part', text }));
    }
    return [];
  }

  getTreeItem(node: TaskNode): vscode.TreeItem {
    switch (node.type) {
      case 'task': {
        const { task } = node;
        const item = new vscode.TreeItem(
          task.name,
          task.states.length > 0
            ? vscode.TreeItemCollapsibleState.Expanded
            : vscode.TreeItemCollapsibleState.None,
        );
        item.id = `task:${task.name}`;
        item.description = task.summary;
        item.iconPath = task.spawned
          ? new vscode.ThemeIcon('pulse')
          : new vscode.ThemeIcon('circle-slash', new vscode.ThemeColor('disabledForeground'));
        item.tooltip = new vscode.MarkdownString().appendMarkdown(
          [
            `**${task.name}**`,
            task.running ? `- running: ${task.running} of PC samples inside its poll` : '',
            task.runQueued ? `- run-queued: ${task.runQueued} of samples waiting to be polled` : '',
          ]
            .filter((l) => l !== '')
            .join('\n'),
        );
        return item;
      }
      case 'state': {
        const { state } = node;
        const item = new vscode.TreeItem(state.label);
        item.id = `state:${node.task}:${state.label}`;
        const where = state.file ? ` · ${state.file}:${state.line}` : '';
        item.description = `${state.percent.toFixed(1)}%${where}`;
        item.iconPath = new vscode.ThemeIcon('debug-pause');
        item.tooltip = `Parked at ${state.label} in ${state.percent.toFixed(1)}% of samples${
          state.file ? `\n${state.file}:${state.line}` : ''
        }`;
        if (state.file && state.line !== undefined) {
          item.command = {
            command: 'tuningStudio.openSource',
            title: 'Open',
            arguments: [state.file, state.line],
          };
        }
        return item;
      }
      case 'outside': {
        const item = new vscode.TreeItem('Outside tasks', vscode.TreeItemCollapsibleState.Collapsed);
        item.id = 'outside';
        item.iconPath = new vscode.ThemeIcon('symbol-method');
        item.description = node.parts.slice(0, 2).join(', ');
        return item;
      }
      case 'part':
        return new vscode.TreeItem(node.text);
      case 'line': {
        const item = new vscode.TreeItem(node.text);
        item.iconPath = new vscode.ThemeIcon('info');
        return item;
      }
    }
  }
}
