import * as vscode from 'vscode';
import * as path from 'node:path';
import { parseJsonc } from './jsonc';
import { pickLaunch, probeRsLaunches, ProbeRsLaunch, substituteVariables } from './launchConfig';
import * as settings from './settings';

export interface TuningInputs {
  launches: ProbeRsLaunch[];
  launchError?: string;
  elf?: string;
  elfSource?: string;
}

/** The workspace folder Tuning Studio works in: the first one. */
export function primaryFolder(): vscode.WorkspaceFolder | undefined {
  return vscode.workspace.workspaceFolders?.[0];
}

/** Read only the ELF metadata needed for offline watch suggestions. Never start a build. */
export async function readInputs(folder: vscode.WorkspaceFolder): Promise<TuningInputs> {
  const inputs: TuningInputs = { launches: [] };
  const uri = vscode.Uri.joinPath(folder.uri, '.vscode', 'launch.json');
  try {
    inputs.launches = probeRsLaunches(parseJsonc(new TextDecoder().decode(await vscode.workspace.fs.readFile(uri))));
  } catch (error) {
    if ((error as { code?: string }).code !== 'FileNotFound') {
      inputs.launchError = error instanceof Error ? error.message : String(error);
    }
  }
  const explicit = settings.elf();
  if (explicit) {
    inputs.elf = path.resolve(folder.uri.fsPath, explicit);
    inputs.elfSource = 'tuningStudio.elf';
    return inputs;
  }
  const picked = pickLaunch(inputs.launches, settings.launchConfiguration());
  inputs.launchError ??= picked.error;
  if (picked.launch?.programBinary) {
    const expanded = substituteVariables(picked.launch.programBinary, {
      workspaceFolder: folder.uri.fsPath, env: process.env,
    });
    if (expanded.value !== undefined) {
      inputs.elf = path.resolve(folder.uri.fsPath, expanded.value);
      inputs.elfSource = `launch.json "${picked.launch.name}"`;
    } else inputs.launchError ??= expanded.error;
  }
  return inputs;
}

export async function currentInputs(): Promise<TuningInputs | undefined> {
  const folder = primaryFolder();
  return folder ? readInputs(folder) : undefined;
}
