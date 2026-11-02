import * as vscode from 'vscode';
import { parseCargoConfig } from './cargoConfig';
import { parseJsonc } from './jsonc';
import { probeRsLaunches } from './launchConfig';
import { PlanInputs, resolveRunPlan, RunPlan } from './runPlan';
import { planSettings } from './settings';

/** The workspace folder Tuning Studio works in: the first one. */
export function primaryFolder(): vscode.WorkspaceFolder | undefined {
  return vscode.workspace.workspaceFolders?.[0];
}

async function readText(uri: vscode.Uri): Promise<string | undefined> {
  try {
    return new TextDecoder().decode(await vscode.workspace.fs.readFile(uri));
  } catch {
    return undefined;
  }
}

/** Read launch.json, `.cargo/config.toml` and `Cargo.toml` presence for the plan. */
export async function readPlanInputs(folder: vscode.WorkspaceFolder): Promise<PlanInputs> {
  const base = folder.uri;
  const inputs: PlanInputs = {
    settings: planSettings(),
    workspaceFolder: base.fsPath,
    env: process.env,
    launches: [],
    hasCargoToml: (await readText(vscode.Uri.joinPath(base, 'Cargo.toml'))) !== undefined,
  };
  const launch = await readText(vscode.Uri.joinPath(base, '.vscode', 'launch.json'));
  if (launch !== undefined) {
    try {
      inputs.launches = probeRsLaunches(parseJsonc(launch));
    } catch (error) {
      inputs.launchError = error instanceof Error ? error.message : String(error);
    }
  }
  const cargo =
    (await readText(vscode.Uri.joinPath(base, '.cargo', 'config.toml'))) ??
    (await readText(vscode.Uri.joinPath(base, '.cargo', 'config')));
  if (cargo !== undefined) {
    inputs.cargo = parseCargoConfig(cargo);
  }
  return inputs;
}

export async function currentPlan(): Promise<RunPlan | undefined> {
  const folder = primaryFolder();
  return folder ? resolveRunPlan(await readPlanInputs(folder)) : undefined;
}
