import * as vscode from 'vscode';
import { cargoConfigDirectories, discoverCargoConfig } from './cargoConfig';
import * as path from 'node:path';
import { homedir } from 'node:os';
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

/** Read launch.json and Cargo configuration inherited by this working directory. */
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
  try {
    inputs.cargo = await discoverCargoConfig(
      base.fsPath,
      cargoHome(base.fsPath),
      async file => {
        try {
          return new TextDecoder().decode(await vscode.workspace.fs.readFile(vscode.Uri.file(file)));
        } catch (error) {
          if ((error as { code?: string }).code === 'FileNotFound') return undefined;
          throw new Error(`Could not read ${file}: ${error instanceof Error ? error.message : String(error)}`);
        }
      },
    );
  } catch (error) {
    inputs.cargoError = error instanceof Error ? error.message : String(error);
  }
  return inputs;
}

export async function currentPlan(): Promise<RunPlan | undefined> {
  const folder = primaryFolder();
  return folder ? resolveRunPlan(await readPlanInputs(folder)) : undefined;
}

function cargoHome(cwd: string): string {
  return process.env.CARGO_HOME ? path.resolve(cwd, process.env.CARGO_HOME) : path.join(homedir(), '.cargo');
}

export function inheritedCargoDirectories(folder: vscode.WorkspaceFolder): string[] {
  return cargoConfigDirectories(folder.uri.fsPath, cargoHome(folder.uri.fsPath));
}
