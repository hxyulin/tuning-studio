import * as vscode from 'vscode';

const SECTION = 'tuningStudio';

function section(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration(SECTION);
}

function clamp(value: number, min: number, max: number, fallback: number): number {
  return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;
}

export function launchConfiguration(): string {
  return section().get<string>('launchConfiguration', '').trim();
}

export function elf(): string {
  return section().get<string>('elf', '').trim();
}

export function executable(): string {
  return section().get<string>('executable', 'tuning-studio').trim() || 'tuning-studio';
}

export function probe(): string {
  return section().get<string>('probe', '').trim();
}

export function pollInterval(): number {
  return clamp(section().get<number>('pollInterval', 200), 20, 10000, 200);
}

export function tasksDuration(): number {
  return clamp(section().get<number>('tasksDuration', 200), 50, 10000, 200);
}

export function plotWindow(): number {
  return clamp(section().get<number>('plotWindow', 30), 1, 3600, 30);
}

export function plotCapacity(): number {
  return Math.round(clamp(section().get<number>('plotCapacity', 6000), 100, 100000, 6000));
}

export async function update(key: string, value: unknown): Promise<void> {
  await section().update(key, value, vscode.ConfigurationTarget.Workspace);
}

export function affects(event: vscode.ConfigurationChangeEvent, key?: string): boolean {
  return event.affectsConfiguration(key ? `${SECTION}.${key}` : SECTION);
}
