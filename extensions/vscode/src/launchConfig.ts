/** The fields Tuning Studio takes from a probe-rs launch configuration. */
export interface ProbeRsLaunch {
  name: string;
  request: string;
  chip?: string;
  programBinary?: string;
  preLaunchTask?: string;
}

/** Every `probe-rs-debug` configuration in a parsed `launch.json`. */
export function probeRsLaunches(launchJson: unknown): ProbeRsLaunch[] {
  if (!isObject(launchJson) || !Array.isArray(launchJson.configurations)) {
    return [];
  }
  const found: ProbeRsLaunch[] = [];
  for (const entry of launchJson.configurations) {
    if (!isObject(entry) || entry.type !== 'probe-rs-debug' || typeof entry.name !== 'string') {
      continue;
    }
    const cores = Array.isArray(entry.coreConfigs) ? entry.coreConfigs : [];
    const first = isObject(cores[0]) ? cores[0] : {};
    found.push({
      name: entry.name,
      request: typeof entry.request === 'string' ? entry.request : 'launch',
      chip: nonEmpty(entry.chip),
      programBinary: nonEmpty(first.programBinary),
      preLaunchTask: nonEmpty(entry.preLaunchTask),
    });
  }
  return found;
}

/**
 * The configuration named in settings, or else the first `launch` request, or
 * else the first one.
 */
export function pickLaunch(
  launches: readonly ProbeRsLaunch[],
  name: string,
): { launch?: ProbeRsLaunch; error?: string } {
  if (name !== '') {
    const launch = launches.find((l) => l.name === name);
    return launch
      ? { launch }
      : { error: `launch.json has no probe-rs-debug configuration named "${name}"` };
  }
  return { launch: launches.find((l) => l.request === 'launch') ?? launches[0] };
}

export interface VariableContext {
  workspaceFolder: string;
  env: Readonly<Record<string, string | undefined>>;
}

/**
 * Expand the VS Code variables a launch configuration path uses. Unknown
 * variables are an error rather than a guess.
 */
export function substituteVariables(
  value: string,
  context: VariableContext,
): { value?: string; error?: string } {
  let error: string | undefined;
  const expanded = value.replace(/\$\{([^}]+)\}/g, (whole, name: string) => {
    if (name === 'workspaceFolder' || name === 'workspaceRoot') {
      return context.workspaceFolder;
    }
    if (name === 'workspaceFolderBasename') {
      return basename(context.workspaceFolder);
    }
    if (name === 'pathSeparator' || name === '/') {
      return '/';
    }
    if (name.startsWith('env:')) {
      return context.env[name.slice(4)] ?? '';
    }
    error ??= `${whole} is not supported in programBinary; set tuningStudio.elf instead`;
    return whole;
  });
  return error ? { error } : { value: expanded };
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter((p) => p !== '');
  return parts[parts.length - 1] ?? '';
}

function nonEmpty(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== '' ? value : undefined;
}

export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
