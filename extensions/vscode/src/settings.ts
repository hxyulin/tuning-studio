import * as vscode from 'vscode';
import { PlanSettings, RunMode } from './runPlan';

const SECTION = 'tuningStudio';

function section(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration(SECTION);
}

function clamp(value: number, min: number, max: number, fallback: number): number {
  return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;
}

export function planSettings(): PlanSettings {
  const c = section();
  const mode = c.get<string>('runMode', 'auto');
  const args = c.get<unknown>('cargoArgs', []);
  return {
    executable: executable(),
    probe: probe(),
    runMode: (['auto', 'cargo', 'task'].includes(mode) ? mode : 'auto') as RunMode,
    launchConfiguration: c.get<string>('launchConfiguration', ''),
    chip: c.get<string>('chip', ''),
    elf: c.get<string>('elf', ''),
    buildTask: c.get<string>('buildTask', ''),
    cargoTarget: c.get<string>('cargoTarget', ''),
    cargoArgs: Array.isArray(args) ? args.filter((a): a is string => typeof a === 'string') : [],
  };
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
