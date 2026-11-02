import * as path from 'node:path';
import * as vscode from 'vscode';
import { RunPlan, Sourced } from './runPlan';
import { SessionController } from './session';
import { probe } from './settings';

type SessionNode =
  | { type: 'state' }
  | { type: 'probe' }
  | { type: 'run' }
  | { type: 'field'; label: string; field: Sourced; display: string }
  | { type: 'note'; text: string; problem: boolean };

/** Session: the session's state, the probe, and how Run starts firmware. */
export class SessionTree implements vscode.TreeDataProvider<SessionNode> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  plan?: RunPlan;
  folder?: string;

  constructor(
    private readonly session: SessionController,
    private readonly unsupported: string | undefined,
  ) {}

  refresh(): void {
    this.changed.fire();
  }

  getChildren(node?: SessionNode): SessionNode[] {
    if (!node) {
      return [{ type: 'state' }, { type: 'probe' }, { type: 'run' }];
    }
    if (node.type !== 'run' || !this.plan) {
      return [];
    }
    const plan = this.plan;
    const nodes: SessionNode[] = [];
    const field = (label: string, value: Sourced | undefined, display?: string) => {
      if (value) {
        nodes.push({ type: 'field', label, field: value, display: display ?? value.value });
      }
    };
    field('Chip', plan.chip);
    if (plan.mode === 'task') {
      field('ELF', plan.elf, plan.elf && this.relative(plan.elf.value));
      field('Build task', plan.buildTask);
    } else if (plan.mode === 'cargo') {
      field('Target', plan.triple);
    }
    if (plan.launchName && plan.mode === 'task') {
      nodes.push({
        type: 'field',
        label: 'Launch config',
        field: { value: plan.launchName, source: '.vscode/launch.json' },
        display: plan.launchName,
      });
    }
    for (const text of plan.problems) {
      nodes.push({ type: 'note', text, problem: true });
    }
    for (const text of plan.notes) {
      nodes.push({ type: 'note', text, problem: false });
    }
    return nodes;
  }

  getTreeItem(node: SessionNode): vscode.TreeItem {
    switch (node.type) {
      case 'state':
        return this.stateItem();
      case 'probe': {
        const selected = probe();
        const item = new vscode.TreeItem('Probe');
        item.id = 'probe';
        item.description = selected || 'any';
        item.iconPath = new vscode.ThemeIcon('plug');
        item.tooltip = selected
          ? `--probe ${selected}`
          : 'No --probe: commands use the only running session or connected probe.';
        item.command = { command: 'tuningStudio.selectProbe', title: 'Select Probe…' };
        return item;
      }
      case 'run': {
        const item = new vscode.TreeItem('Run', vscode.TreeItemCollapsibleState.Expanded);
        item.id = 'run';
        item.iconPath = new vscode.ThemeIcon('rocket');
        const plan = this.plan;
        item.description = !plan
          ? 'no workspace folder'
          : plan.mode === 'cargo'
            ? 'cargo run'
            : plan.mode === 'task'
              ? plan.buildTask
                ? 'build task, then tuning-studio run'
                : 'tuning-studio run'
              : 'not configured';
        item.tooltip = this.commandTooltip();
        return item;
      }
      case 'field': {
        const item = new vscode.TreeItem(node.label);
        item.description = node.display;
        item.tooltip = `${node.field.value}\nFrom ${node.field.source}`;
        return item;
      }
      case 'note': {
        const item = new vscode.TreeItem(node.text);
        item.iconPath = node.problem
          ? new vscode.ThemeIcon('error', new vscode.ThemeColor('problemsErrorIcon.foreground'))
          : new vscode.ThemeIcon('info');
        item.tooltip = node.text;
        return item;
      }
    }
  }

  private stateItem(): vscode.TreeItem {
    const item = new vscode.TreeItem('');
    item.id = 'state';
    if (this.unsupported) {
      item.label = this.unsupported;
      item.iconPath = new vscode.ThemeIcon('error', new vscode.ThemeColor('problemsErrorIcon.foreground'));
      return item;
    }
    const state = this.session.state;
    switch (state.kind) {
      case 'unknown':
        item.label = 'Checking for a session';
        item.iconPath = new vscode.ThemeIcon('loading~spin');
        break;
      case 'none':
        item.label = 'No session';
        item.iconPath = new vscode.ThemeIcon('circle-outline');
        item.tooltip =
          'Run Firmware starts one. A tuning-studio run or attach started in a terminal shows here too.';
        item.command = { command: 'tuningStudio.run', title: 'Run Firmware' };
        break;
      case 'building':
        item.label = 'Building';
        item.description = state.task;
        item.iconPath = new vscode.ThemeIcon('loading~spin');
        break;
      case 'starting':
        item.label = 'Starting';
        item.description = state.detail;
        item.tooltip = 'Waiting until `tuning-studio status` reaches the new session.';
        item.iconPath = new vscode.ThemeIcon('loading~spin');
        break;
      case 'ready': {
        const halted = state.status.startsWith('halted') || state.status === 'locked up';
        item.label = halted ? 'Halted' : state.status === 'running' ? 'Running' : capitalize(state.status);
        item.description = state.owned ? 'started here' : 'external';
        item.tooltip = `Core: ${state.status}\n${
          state.owned ? 'Started by Run; Stop ends it.' : 'Started outside VS Code; stop it where it runs.'
        }`;
        item.iconPath = halted
          ? new vscode.ThemeIcon('debug-pause', new vscode.ThemeColor('debugIcon.pauseForeground'))
          : new vscode.ThemeIcon('pass-filled', new vscode.ThemeColor('testing.iconPassed'));
        break;
      }
      case 'error':
        item.label = state.failure.message;
        item.tooltip = state.failure.message;
        item.iconPath = new vscode.ThemeIcon('error', new vscode.ThemeColor('problemsErrorIcon.foreground'));
        if (state.failure.kind === 'several-sessions') {
          item.command = { command: 'tuningStudio.selectProbe', title: 'Select Probe…' };
        }
        break;
    }
    return item;
  }

  private commandTooltip(): string | undefined {
    const command = this.plan?.command;
    if (!command) {
      return undefined;
    }
    if (command.kind === 'cargo') {
      const env = Object.entries(command.env).map(([k, v]) => `${k}="${v}"`);
      return `${env.join(' ')} cargo ${command.args.join(' ')}`;
    }
    const run = `${command.executable} ${command.args.join(' ')}`;
    return command.buildTask ? `Task "${command.buildTask}", then\n${run}` : run;
  }

  private relative(file: string): string {
    if (!this.folder) {
      return file;
    }
    const relative = path.relative(this.folder, file);
    return relative.startsWith('..') ? file : relative;
  }
}

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}
