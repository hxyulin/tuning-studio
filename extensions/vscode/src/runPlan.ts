import * as path from 'node:path';
import { CargoConfig, chipFromRunner, runnerEnvName } from './cargoConfig';
import { ProbeRsLaunch, pickLaunch, substituteVariables } from './launchConfig';

export type RunMode = 'auto' | 'cargo' | 'task';

export interface PlanSettings {
  executable: string;
  probe: string;
  runMode: RunMode;
  launchConfiguration: string;
  chip: string;
  elf: string;
  buildTask: string;
  cargoTarget: string;
  cargoArgs: string[];
}

export interface PlanInputs {
  settings: PlanSettings;
  workspaceFolder: string;
  env: Readonly<Record<string, string | undefined>>;
  /** probe-rs configurations from launch.json, empty when there is none. */
  launches: ProbeRsLaunch[];
  /** Why launch.json could not be read, when it exists but does not parse. */
  launchError?: string;
  cargo?: CargoConfig;
  hasCargoToml: boolean;
}

/** A resolved value and where it came from, shown next to it in the Session view. */
export interface Sourced {
  value: string;
  source: string;
}

export type RunCommand =
  | {
      kind: 'cargo';
      args: string[];
      env: Record<string, string>;
      triple: string;
    }
  | {
      kind: 'task';
      buildTask?: string;
      executable: string;
      args: string[];
    };

export interface RunPlan {
  mode?: 'cargo' | 'task';
  launchName?: string;
  chip?: Sourced;
  elf?: Sourced;
  buildTask?: Sourced;
  triple?: Sourced;
  /** Problems that stop Run. */
  problems: string[];
  /** Facts the user should see before Run, such as a missing build task. */
  notes: string[];
  command?: RunCommand;
}

/**
 * Decide how Run starts a session. Explicit settings win, then the probe-rs
 * launch configuration, then the Cargo runner.
 */
export function resolveRunPlan(inputs: PlanInputs): RunPlan {
  const { settings } = inputs;
  const plan: RunPlan = { problems: [], notes: [] };
  if (inputs.launchError) {
    plan.notes.push(`launch.json: ${inputs.launchError}`);
  }

  const picked = pickLaunch(inputs.launches, settings.launchConfiguration.trim());
  if (picked.error) {
    plan.problems.push(picked.error);
  }
  const launch = picked.launch;
  plan.launchName = launch?.name;
  const launchSource = launch ? `launch.json "${launch.name}"` : '';

  const triple = settings.cargoTarget.trim()
    ? { value: settings.cargoTarget.trim(), source: 'settings' }
    : inputs.cargo?.buildTargets.length === 1
      ? { value: inputs.cargo.buildTargets[0], source: '.cargo/config.toml' }
      : undefined;
  plan.triple = triple;
  const runner = triple ? inputs.cargo?.runners.get(triple.value) : undefined;
  const runnerChip = runner ? chipFromRunner(runner) : undefined;

  if (settings.chip.trim()) {
    plan.chip = { value: settings.chip.trim(), source: 'settings' };
  } else if (launch?.chip) {
    plan.chip = { value: launch.chip, source: launchSource };
  } else if (runnerChip) {
    plan.chip = { value: runnerChip, source: 'Cargo runner' };
  }

  if (settings.elf.trim()) {
    plan.elf = {
      value: path.resolve(inputs.workspaceFolder, settings.elf.trim()),
      source: 'settings',
    };
  } else if (launch?.programBinary) {
    const expanded = substituteVariables(launch.programBinary, {
      workspaceFolder: inputs.workspaceFolder,
      env: inputs.env,
    });
    if (expanded.value !== undefined) {
      plan.elf = {
        value: path.resolve(inputs.workspaceFolder, expanded.value),
        source: launchSource,
      };
    } else if (expanded.error) {
      plan.notes.push(expanded.error);
    }
  }

  if (settings.buildTask.trim()) {
    plan.buildTask = { value: settings.buildTask.trim(), source: 'settings' };
  } else if (launch?.preLaunchTask) {
    plan.buildTask = { value: launch.preLaunchTask, source: launchSource };
  }

  if (settings.runMode === 'cargo' || settings.runMode === 'task') {
    plan.mode = settings.runMode;
  } else if (plan.elf) {
    plan.mode = 'task';
  } else if (inputs.hasCargoToml && triple) {
    plan.mode = 'cargo';
  }

  if (!plan.chip) {
    plan.problems.push(
      'No chip: set tuningStudio.chip, add "chip" to a probe-rs launch configuration, or pass --chip in the Cargo runner',
    );
  }
  const executable = settings.executable.trim() || 'tuning-studio';
  const probeArgs = settings.probe.trim() ? ['--probe', settings.probe.trim()] : [];

  if (plan.mode === undefined) {
    plan.problems.push(
      'No way to run: set tuningStudio.elf or a launch configuration with programBinary, or set build.target in .cargo/config.toml',
    );
  } else if (plan.mode === 'task') {
    if (!plan.elf) {
      plan.problems.push(
        'Task mode needs an ELF: set tuningStudio.elf or programBinary in the launch configuration',
      );
    }
    if (!plan.buildTask) {
      plan.notes.push('No build task: Run flashes the ELF as it is on disk');
    }
    if (plan.chip && plan.elf) {
      plan.command = {
        kind: 'task',
        buildTask: plan.buildTask?.value,
        executable,
        args: ['run', '--chip', plan.chip.value, ...probeArgs, plan.elf.value],
      };
    }
  } else {
    if (settings.cargoArgs.some((arg) => arg === '--target' || arg.startsWith('--target='))) {
      plan.problems.push('Set tuningStudio.cargoTarget instead of passing --target in cargoArgs');
    }
    if (!inputs.hasCargoToml) {
      plan.problems.push('Cargo mode needs a Cargo.toml in the workspace folder');
    }
    if (!triple) {
      plan.problems.push(
        inputs.cargo && inputs.cargo.buildTargets.length > 1
          ? 'build.target lists several triples: set tuningStudio.cargoTarget'
          : 'No target triple: set build.target in .cargo/config.toml or tuningStudio.cargoTarget',
      );
    }
    if (plan.chip && triple) {
      // Cargo splits a runner from the environment on whitespace, without quoting.
      const words = [executable, 'run', '--chip', plan.chip.value, ...probeArgs];
      const spaced = words.find((w) => /\s/.test(w));
      if (spaced !== undefined) {
        plan.problems.push(
          `Cargo cannot pass "${spaced}" in a runner because it contains whitespace; use task mode`,
        );
      } else {
        plan.command = {
          kind: 'cargo',
          args: ['run', '--target', triple.value, ...settings.cargoArgs],
          env: { [runnerEnvName(triple.value)]: words.join(' ') },
          triple: triple.value,
        };
      }
    }
  }
  if (plan.problems.length > 0) {
    plan.command = undefined;
  }
  return plan;
}
