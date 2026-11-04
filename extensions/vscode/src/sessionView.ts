import * as path from 'node:path';
import * as vscode from 'vscode';
import { SessionController } from './session';
import { probe } from './settings';
import { TuningInputs } from './workspaceInputs';

type SessionNode = 'state' | 'probe' | 'elf' | 'error';

/** Connection status and the ELF used for offline suggestions. */
export class SessionTree implements vscode.TreeDataProvider<SessionNode> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  inputs?: TuningInputs;
  folder?: string;

  constructor(private readonly session: SessionController, private readonly unsupported: string | undefined) {}

  refresh(): void { this.changed.fire(); }

  getChildren(node?: SessionNode): SessionNode[] {
    if (node) return [];
    return ['state', 'probe', ...(this.inputs?.elf ? ['elf' as const] : []),
      ...(this.inputs?.launchError ? ['error' as const] : [])];
  }

  getTreeItem(node: SessionNode): vscode.TreeItem {
    const item = new vscode.TreeItem('');
    item.id = node;
    if (node === 'probe') {
      item.label = 'Probe';
      item.description = probe() || 'any';
      item.iconPath = new vscode.ThemeIcon('plug');
      item.tooltip = 'Select the probe used by an existing tuning-studio session.';
      item.command = { command: 'tuningStudio.selectProbe', title: 'Select Probe…' };
    } else if (node === 'elf') {
      const elf = this.inputs!.elf!;
      item.label = 'ELF';
      item.description = this.folder ? path.relative(this.folder, elf) : elf;
      item.tooltip = `${elf}\nFrom ${this.inputs!.elfSource}. Used for offline suggestions only.`;
    } else if (node === 'error') {
      item.label = this.inputs!.launchError;
      item.iconPath = new vscode.ThemeIcon('warning');
    } else {
      const state = this.session.state;
      if (this.unsupported) {
        item.label = this.unsupported;
        item.iconPath = new vscode.ThemeIcon('error');
      } else if (state.kind === 'unknown') {
        item.label = 'Checking session';
        item.iconPath = new vscode.ThemeIcon('loading~spin');
      } else if (state.kind === 'none') {
        item.label = 'No session';
        item.iconPath = new vscode.ThemeIcon('circle-outline');
        item.tooltip = 'Start tuning-studio run or attach in a terminal. Stop it there with Ctrl+C. Probe-rs debugging cannot share this connection yet.';
      } else if (state.kind === 'ready') {
        item.label = state.status;
        item.iconPath = new vscode.ThemeIcon(state.status.startsWith('halted') ? 'debug-pause' : 'pulse');
        item.tooltip = 'Connected to a tuning-studio session. Its terminal controls its lifetime.';
      } else {
        item.label = state.failure.message;
        item.tooltip = state.failure.message;
        item.iconPath = new vscode.ThemeIcon('error');
        if (state.failure.kind === 'several-sessions') {
          item.command = { command: 'tuningStudio.selectProbe', title: 'Select Probe…' };
        }
      }
    }
    return item;
  }
}
