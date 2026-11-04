import * as vscode from 'vscode';
import { InspectValue, singleLine, ValueKind } from './inspect';
import { LiveWatch } from './liveWatch';
import { SessionController } from './session';

export type WatchNode =
  | { type: 'group'; group: 'declared' | 'watched' }
  | { type: 'location'; location: string }
  | { type: 'value'; value: InspectValue; parent: string };

const KIND_ICON: Record<ValueKind, string> = {
  param: 'symbol-parameter',
  watch_mut: 'symbol-variable',
  watch: 'eye',
  raw: 'symbol-field',
};

const KIND_NAME: Record<ValueKind, string> = {
  param: 'Param',
  watch_mut: 'watch_mut!',
  watch: 'watch!',
  raw: 'memory',
};

/** Live Watch: declared values and user locations, and their status. */
export class WatchTree implements vscode.TreeDataProvider<WatchNode> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;

  constructor(
    private readonly watch: LiveWatch,
    private readonly session: SessionController,
  ) {}

  refresh(): void {
    this.changed.fire();
  }

  /** The tree's message line: why values are missing or stale. */
  message(): string | undefined {
    const state = this.session.state;
    if (state.kind === 'error') {
      return state.failure.message;
    }
    if (state.kind !== 'ready') {
      return 'No running session';
    }
    if (this.watch.paused) {
      return 'Polling paused';
    }
    const error = this.watch.declaredError;
    if (error) {
      const since = this.watch.lastSuccess
        ? `; last read ${this.watch.lastSuccess.toLocaleTimeString()}`
        : '';
      return `${error}${since}`;
    }
    return undefined;
  }

  getChildren(node?: WatchNode): WatchNode[] {
    if (!node) {
      return [
        { type: 'group', group: 'declared' },
        { type: 'group', group: 'watched' },
      ];
    }
    if (node.type === 'group') {
      return node.group === 'declared'
        ? this.watch.declared.map((value) => ({ type: 'value', value, parent: 'declared' }))
        : this.watch.watched.map((location) => ({ type: 'location', location }));
    }
    if (node.type === 'location') {
      const values = this.watch.watchedValues.get(node.location) ?? [];
      return this.watch.watchedError || single(values)
        ? []
        : values.map((value) => ({ type: 'value', value, parent: node.location }));
    }
    return [];
  }

  getTreeItem(node: WatchNode): vscode.TreeItem {
    if (node.type === 'group') {
      const declared = node.group === 'declared';
      const count = declared ? this.watch.declared.length : this.watch.watched.length;
      const item = new vscode.TreeItem(
        declared ? 'Declared' : 'Watched',
        vscode.TreeItemCollapsibleState.Expanded,
      );
      item.id = `group:${node.group}`;
      item.description = count > 0 ? String(count) : undefined;
      item.contextValue = `group ${node.group}`;
      if (declared && count === 0) {
        item.collapsibleState = vscode.TreeItemCollapsibleState.None;
        item.description = this.session.isReady && this.watch.lastSuccess ? 'none' : undefined;
      }
      if (!declared && count === 0) {
        item.collapsibleState = vscode.TreeItemCollapsibleState.None;
        item.description = 'Add Watch…';
        item.command = { command: 'tuningStudio.addWatch', title: 'Add Watch…' };
      }
      return item;
    }
    if (node.type === 'location') {
      const values = this.watch.watchedValues.get(node.location) ?? [];
      const only = this.watch.watchedError ? undefined : single(values);
      if (only) {
        const item = this.valueItem(only, `watched:${node.location}`, node.location);
        item.contextValue += ' watched';
        return item;
      }
      const item = new vscode.TreeItem(
        node.location,
        values.length > 0
          ? vscode.TreeItemCollapsibleState.Collapsed
          : vscode.TreeItemCollapsibleState.None,
      );
      item.id = `watched:${node.location}`;
      item.contextValue = 'watched';
      const error =
        this.watch.watchedError ??
        (this.session.isReady && values.length === 0 && this.watch.lastSuccess
          ? 'no value returned'
          : undefined);
      if (error) {
        item.description = error;
        item.iconPath = new vscode.ThemeIcon('warning', new vscode.ThemeColor('problemsWarningIcon.foreground'));
        item.tooltip = `${node.location}\n${error}`;
      } else {
        item.iconPath = new vscode.ThemeIcon('symbol-structure');
        item.description = values.length > 0 ? `${values.length} values` : undefined;
      }
      return item;
    }
    return this.valueItem(node.value, `${node.parent}:${node.value.path}`, node.value.path);
  }

  private valueItem(value: InspectValue, id: string, label: string): vscode.TreeItem {
    const item = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.None);
    item.id = id;
    const parts = ['value'];
    if (value.writable) {
      parts.push('writable');
    }
    item.contextValue = parts.join(' ');
    if (value.error !== undefined) {
      item.description = value.error;
      item.iconPath = new vscode.ThemeIcon('warning', new vscode.ThemeColor('problemsWarningIcon.foreground'));
    } else {
      const firstLine = value.display.split(/\r?\n/, 1)[0];
      const equals = firstLine.indexOf(' = ');
      item.description = equals < 0
        ? value.type
        : singleLine(firstLine.slice(equals + 3).split(' (', 1)[0]);
      item.iconPath = new vscode.ThemeIcon(KIND_ICON[value.kind]);
    }
    const tooltip = new vscode.MarkdownString();
    tooltip.appendMarkdown(`**${escapeMarkdown(value.path)}**\n\n`);
    tooltip.appendMarkdown(
      `\`${escapeMarkdown(value.type)}\` · ${KIND_NAME[value.kind]}${value.writable ? ' · writable' : ''}\n`,
    );
    tooltip.appendCodeblock(value.error ?? value.display, 'text');
    item.tooltip = tooltip;
    return item;
  }
}

/** A location that read as exactly one value is shown as that value. */
export function single(values: readonly InspectValue[]): InspectValue | undefined {
  return values.length === 1 ? values[0] : undefined;
}

function escapeMarkdown(text: string): string {
  return text.replace(/[\\`*_{}[\]()#+\-.!|<>]/g, '\\$&');
}
