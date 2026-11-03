import * as path from 'node:path';
import * as vscode from 'vscode';
import { Cli } from './cli';
import { Failure, parseProbeList, setArgs, tasksArgs } from './cliText';
import { InspectValue, flattenValues } from './inspect';
import { LiveWatch } from './liveWatch';
import { MAX_SERIES, PlotModel, PlotViewProvider } from './plotView';
import { SessionController, SessionState } from './session';
import { SessionTree } from './sessionView';
import * as settings from './settings';
import { parseTasksOutput } from './tasksOutput';
import { TasksTree } from './tasksView';
import { single, WatchNode, WatchTree } from './watchView';
import { WatchPanel } from './watchPanel';
import { insertMutableWatch, suggestMutableWatch } from './sourceSuggestion';
import { currentPlan, primaryFolder, readPlanInputs } from './workspaceInputs';

const PLOT_WINDOW_KEY = 'tuningStudio.plotWindow';


export function activate(context: vscode.ExtensionContext): void {
  const log = vscode.window.createOutputChannel('Tuning Studio');
  const tasksLog = vscode.window.createOutputChannel('Tuning Studio Tasks');
  const unsupported =
    process.platform === 'win32'
      ? 'tuning-studio sessions use Unix sockets; Windows is not supported yet'
      : undefined;

  const cli = new Cli(log, () => primaryFolder()?.uri.fsPath);
  const session = new SessionController(cli, log);
  const watch = new LiveWatch(cli, session, context.workspaceState);
  const plot = new PlotModel(context.workspaceState);
  plot.windowSeconds = context.workspaceState.get<number>(PLOT_WINDOW_KEY) ?? settings.plotWindow();

  const watchTree = new WatchTree(watch, session);
  const sessionTree = new SessionTree(session, unsupported);
  const tasksTree = new TasksTree();
  const plotView = new PlotViewProvider(context.extensionUri, plot, (p) => void setPlotted(p, false));

  const sessionView = vscode.window.createTreeView('tuningStudio.session', {
    treeDataProvider: sessionTree,
  });
  const watchView = new WatchPanel(context.extensionUri, watch, async (value, input) => {
    const result = await cli.run(setArgs(settings.probe(), value.path, input), { timeoutMs: 10000, log: true });
    if (!result.ok) {
      if (result.failure.kind === 'no-session') session.markLost();
      return { ok: false, text: noSession(result.failure) };
    }
    return { ok: true, text: result.stdout.trim() || 'Write sent. The next read shows the current value.' };
  }, p => { void suggestMutableWatch(p); });
  const tasksView = vscode.window.createTreeView('tuningStudio.tasks', {
    treeDataProvider: tasksTree,
  });
  const statusItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  statusItem.name = 'Tuning Studio';

  for (const [key, value] of [
    ['tuningStudio.owned', false],
    ['tuningStudio.busy', false],
    ['tuningStudio.pollingPaused', false],
    ['tuningStudio.plotPaused', false],
    ['tuningStudio.tasksSampled', false],
    ['tuningStudio.tasksSampling', false],
  ] as const) {
    void vscode.commands.executeCommand('setContext', key, value);
  }

  // ---- state to views -----------------------------------------------------

  const updateMonitoring = () => {
    if (unsupported) {
      return;
    }
    session.setMonitoring(
      sessionView.visible || watchView.visible || tasksView.visible || plotView.visible,
    );
  };

  const plotStatus = (): string => {
    const state = session.state;
    if (state.kind === 'error') {
      return state.failure.message;
    }
    if (state.kind !== 'ready') {
      return 'No running session';
    }
    if (tasksTree.sampling !== undefined) {
      return 'Paused while tasks are sampled';
    }
    if (watch.paused) {
      return 'Polling paused';
    }
    const error = watch.declaredError ?? watch.watchedError;
    return error ? `Read failed: ${error}` : '';
  };

  const updateWatchView = () => {
    watchTree.refresh();
    watchView.refresh(watchTree.message() ?? '');
    const choices = [...new Set(flattenValues([...watch.declared, ...watch.watchedValues.values()].flat()).filter(v => v.number !== undefined || /^(?:[ui](?:8|16|32|64|128|size)|f(?:32|64))$/.test(v.type)).map(v => v.path))];
    if (JSON.stringify(choices) !== JSON.stringify(plot.choices)) { plot.choices = choices; plotView.reset(); }
    plotView.setStatus(plotStatus());
  };

  const updateStatusItem = (state: SessionState) => {
    if (unsupported) {
      statusItem.hide();
      return;
    }
    statusItem.backgroundColor = undefined;
    switch (state.kind) {
      case 'unknown':
      case 'none':
        statusItem.text = '$(play) Tuning Studio';
        statusItem.tooltip = 'Run firmware with tuning-studio';
        statusItem.command = 'tuningStudio.run';
        break;
      case 'building':
        statusItem.text = '$(loading~spin) Building';
        statusItem.tooltip = `Running task "${state.task}"`;
        statusItem.command = 'tuningStudio.session.focus';
        break;
      case 'starting':
        statusItem.text = '$(loading~spin) Starting session';
        statusItem.tooltip = 'Waiting for the tuning-studio session to answer';
        statusItem.command = 'tuningStudio.session.focus';
        break;
      case 'ready':
        statusItem.text = state.status.startsWith('halted')
          ? '$(debug-pause) Halted'
          : '$(pulse) Running';
        statusItem.tooltip = `tuning-studio: ${state.status}${state.owned ? '' : ' (external session)'}`;
        statusItem.command = 'tuningStudio.session.focus';
        break;
      case 'error':
        statusItem.text = '$(error) Tuning Studio';
        statusItem.tooltip = state.failure.message;
        statusItem.command = 'tuningStudio.session.focus';
        statusItem.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
        break;
    }
    statusItem.show();
  };

  let planGeneration = 0;
  const refreshPlan = async () => {
    const generation = ++planGeneration;
    const plan = await currentPlan();
    if (generation !== planGeneration) {
      return;
    }
    sessionTree.plan = plan;
    sessionTree.folder = primaryFolder()?.uri.fsPath;
    sessionTree.refresh();
  };

  const setPlotted = async (target: string, plotted: boolean) => {
    if (!(await plot.set(target, plotted))) {
      void vscode.window.showWarningMessage(`The plot shows at most ${MAX_SERIES} values.`);
    }
    watchTree.refresh();
  };

  const valueOf = (node: WatchNode | undefined): InspectValue | undefined => {
    if (!node) {
      return undefined;
    }
    if (node.type === 'value') {
      return node.value;
    }
    if (node.type === 'location') {
      return single(watch.watchedValues.get(node.location) ?? []);
    }
    return undefined;
  };

  const noSession = (failure: Failure) =>
    failure.kind === 'no-session' ? 'No running session' : failure.message;

  // ---- commands -----------------------------------------------------------

  const command = (id: string, handler: (...args: never[]) => unknown) =>
    context.subscriptions.push(vscode.commands.registerCommand(id, handler));

  command('tuningStudio.run', async () => {
    if (unsupported) {
      void vscode.window.showErrorMessage(unsupported);
      return;
    }
    const folder = primaryFolder();
    const plan = await currentPlan();
    await refreshPlan();
    if (!folder || !plan) {
      void vscode.window.showErrorMessage('Open the firmware folder to run it.');
      return;
    }
    if (!plan.command) {
      const choice = await vscode.window.showErrorMessage(
        plan.problems[0] ?? 'Run is not configured',
        'Open Settings',
      );
      if (choice) {
        void vscode.commands.executeCommand('workbench.action.openWorkspaceSettings', 'tuningStudio');
      }
      return;
    }
    try {
      await session.run(plan.command, folder);
    } catch (error) {
      void vscode.window.showErrorMessage(`Run failed: ${error instanceof Error ? error.message : String(error)}`);
      await session.check();
    }
  });

  command('tuningStudio.stop', () => session.stop());

  command('tuningStudio.refreshSession', async () => {
    await refreshPlan();
    await session.check();
  });

  command('tuningStudio.showLog', () => log.show());

  command('tuningStudio.selectProbe', async () => {
    const result = await cli.run(['list'], { timeoutMs: 10000, log: true });
    if (!result.ok) {
      void vscode.window.showErrorMessage(`Could not list probes: ${result.failure.message}`);
      return;
    }
    const listed = parseProbeList(result.stdout);
    const state = session.state;
    const busy =
      state.kind === 'error' && state.failure.kind === 'several-sessions'
        ? (state.failure.probes ?? [])
        : [];
    const current = settings.probe();
    type Item = vscode.QuickPickItem & { selector: string };
    const items: Item[] = [
      {
        label: 'Any probe',
        description: current === '' ? 'current' : undefined,
        detail: 'No --probe: use the only running session or connected probe',
        selector: '',
      },
      ...listed.map((p) => ({
        label: p.selector,
        description: [
          p.identifier,
          busy.includes(p.selector) ? 'session running' : '',
          current === p.selector ? 'current' : '',
        ]
          .filter((s) => s !== '')
          .join(' · '),
        selector: p.selector,
      })),
      ...busy
        .filter((s) => !listed.some((p) => p.selector === s))
        .map((s) => ({ label: s, description: 'session running', selector: s })),
    ];
    const picked = await vscode.window.showQuickPick(items, {
      title: 'Probe',
      placeHolder: listed.length === 0 ? 'No probes found' : 'Probe for every tuning-studio command',
    });
    if (picked) {
      await settings.update('probe', picked.selector);
      await session.check();
    }
  });

  command('tuningStudio.selectLaunchConfiguration', async () => {
    const folder = primaryFolder();
    if (!folder) {
      return;
    }
    const inputs = await readPlanInputs(folder);
    if (inputs.launches.length === 0) {
      void vscode.window.showInformationMessage(
        inputs.launchError
          ? `launch.json could not be read: ${inputs.launchError}`
          : 'launch.json has no probe-rs-debug configurations.',
      );
      return;
    }
    const current = inputs.settings.launchConfiguration;
    const picked = await vscode.window.showQuickPick(
      [
        { label: 'First launch configuration', name: '', description: current === '' ? 'current' : undefined },
        ...inputs.launches.map((l) => ({
          label: l.name,
          name: l.name,
          description: [l.request, l.chip, current === l.name ? 'current' : undefined]
            .filter(Boolean)
            .join(' · '),
        })),
      ],
      { title: 'Launch configuration to take chip, ELF and build task from' },
    );
    if (picked) {
      await settings.update('launchConfiguration', picked.name);
    }
  });

  command('tuningStudio.addWatch', async () => {
    const location = await vscode.window.showInputBox({
      title: 'Add Watch',
      prompt: 'Symbol path, or address with a type',
      placeHolder: 'CONTROL.yaw.target_rpm    0x24000070:f32',
      validateInput: (v) => (v.trim() === '' ? 'Enter a location' : undefined),
    });
    if (location) {
      await watch.addWatch(location.trim());
    }
  });

  command('tuningStudio.editWatch', async (node?: WatchNode) => {
    if (node?.type !== 'location') {
      return;
    }
    const location = await vscode.window.showInputBox({
      title: 'Edit Watch',
      value: node.location,
      validateInput: (v) => (v.trim() === '' ? 'Enter a location' : undefined),
    });
    if (location && location.trim() !== node.location) {
      await watch.replaceWatch(node.location, location.trim());
    }
  });

  command('tuningStudio.removeWatch', async (node?: WatchNode) => {
    if (node?.type === 'location') {
      await watch.removeWatch(node.location);
    }
  });

  command('tuningStudio.setValue', (node?: WatchNode) => {
    const value = valueOf(node);
    if (value) watchView.focus(value.path);
  });
  command('tuningStudio.insertMutableWatch', insertMutableWatch);
  context.subscriptions.push(vscode.languages.registerCodeActionsProvider('rust', {
    provideCodeActions(document, range) {
      if (range.isEmpty || range.start.line !== range.end.line) return [];
      const action = new vscode.CodeAction('Expose selection with watch_mut!', vscode.CodeActionKind.RefactorRewrite);
      action.command = { command: 'tuningStudio.insertMutableWatch', title: action.title };
      // The command uses the visible editor selection so insertion remains explicit.
      return document.getText(range).trim() ? [action] : [];
    },
  }, { providedCodeActionKinds: [vscode.CodeActionKind.RefactorRewrite] }));

  command('tuningStudio.copyValue', async (node?: WatchNode) => {
    const value = valueOf(node);
    if (value) {
      await vscode.env.clipboard.writeText(value.error ?? value.display);
    }
  });

  command('tuningStudio.copyPath', async (node?: WatchNode) => {
    const text = node?.type === 'location' ? node.location : valueOf(node)?.path;
    if (text) {
      await vscode.env.clipboard.writeText(text);
    }
  });

  command('tuningStudio.pausePolling', () => watch.setPaused(true));
  command('tuningStudio.resumePolling', () => watch.setPaused(false));

  command('tuningStudio.sampleTasks', async () => {
    if (tasksTree.sampling !== undefined) {
      return;
    }
    if (!session.isReady && (await session.check()).kind !== 'ready') {
      void vscode.window.showErrorMessage('Sampling tasks needs a running session.');
      return;
    }
    const duration = settings.tasksDuration();
    tasksTree.sampling = duration;
    tasksView.message = tasksTree.message();
    void vscode.commands.executeCommand('setContext', 'tuningStudio.tasksSampling', true);
    plotView.setStatus(plotStatus());
    try {
      const args = tasksArgs(settings.probe(), duration);
      const result = await watch.block(() => cli.run(args, { timeoutMs: duration + 15000 }));
      const at = new Date();
      tasksLog.appendLine(`--- ${at.toLocaleTimeString()}  tuning-studio ${args.join(' ')}`);
      if (result.ok) {
        tasksLog.appendLine(result.stdout.trimEnd());
        tasksTree.sample = { output: parseTasksOutput(result.stdout), raw: result.stdout, at, durationMs: duration };
        tasksTree.error = undefined;
        void vscode.commands.executeCommand('setContext', 'tuningStudio.tasksSampled', true);
      } else {
        tasksLog.appendLine(`failed: ${result.failure.message}`);
        if (result.failure.kind === 'no-session') {
          session.markLost();
        }
        tasksTree.error = noSession(result.failure);
      }
    } finally {
      tasksTree.sampling = undefined;
      void vscode.commands.executeCommand('setContext', 'tuningStudio.tasksSampling', false);
      tasksTree.refresh();
      tasksView.message = tasksTree.message();
      plotView.setStatus(plotStatus());
    }
  });

  command('tuningStudio.showTasksOutput', () => tasksLog.show());

  command('tuningStudio.openSource', async (file: string, line: number) => {
    const folder = primaryFolder();
    if (typeof file !== 'string' || typeof line !== 'number') {
      return;
    }
    const resolved = path.isAbsolute(file) || !folder ? file : path.join(folder.uri.fsPath, file);
    try {
      const uri = vscode.Uri.file(resolved);
      await vscode.workspace.fs.stat(uri);
      const position = new vscode.Position(Math.max(0, line - 1), 0);
      await vscode.window.showTextDocument(uri, { selection: new vscode.Range(position, position) });
    } catch {
      void vscode.window.showWarningMessage(`${file} is not in this workspace folder.`);
    }
  });

  command('tuningStudio.clearPlot', () => {
    plot.clear();
    plotView.reset();
  });

  command('tuningStudio.setPlotWindow', async () => {
    const options = [5, 10, 30, 60, 120, 300, 600];
    const picked = await vscode.window.showQuickPick(
      options.map((s) => ({
        label: s < 60 ? `${s} s` : `${s / 60} min`,
        seconds: s,
        description: s === plot.windowSeconds ? 'current' : undefined,
      })),
      { title: 'Plot window' },
    );
    if (picked) {
      plot.windowSeconds = picked.seconds;
      await context.workspaceState.update(PLOT_WINDOW_KEY, picked.seconds);
      plotView.refreshView();
    }
  });

  const setFrozen = (frozen: boolean) => {
    plot.frozen = frozen;
    void vscode.commands.executeCommand('setContext', 'tuningStudio.plotPaused', frozen);
    plotView.refreshView();
  };
  command('tuningStudio.pausePlot', () => setFrozen(true));
  command('tuningStudio.resumePlot', () => setFrozen(false));

  // ---- events -------------------------------------------------------------

  const folder = primaryFolder();
  const watchers = folder
    ? ['.vscode/launch.json', '.cargo/config.toml', '.cargo/config', 'Cargo.toml'].map((p) =>
        vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(folder, p)),
      )
    : [];
  for (const watcher of watchers) {
    watcher.onDidChange(() => void refreshPlan());
    watcher.onDidCreate(() => void refreshPlan());
    watcher.onDidDelete(() => void refreshPlan());
  }

  context.subscriptions.push(
    log,
    tasksLog,
    session,
    watch,
    plotView,
    sessionView,
    watchView,
    tasksView,
    statusItem,
    ...watchers,
    vscode.window.registerWebviewViewProvider('tuningStudio.plot', plotView),
    vscode.window.registerWebviewViewProvider('tuningStudio.watch', watchView),
    session.onDidChange((state) => {
      sessionTree.refresh();
      updateWatchView();
      updateStatusItem(state);
    }),
    watch.onDidChange(updateWatchView),
    watch.onDidSample((sample) => plotView.append(sample.time, plot.record(sample))),
    plot.onDidChangeSelection(() => watchTree.refresh()),
    sessionView.onDidChangeVisibility(updateMonitoring),
    watchView.onDidChangeVisibility(updateMonitoring),
    tasksView.onDidChangeVisibility(updateMonitoring),
    plotView.onDidChangeVisibility(updateMonitoring),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (!settings.affects(event)) {
        return;
      }
      if (settings.affects(event, 'plotCapacity')) {
        plot.resize(settings.plotCapacity());
        plotView.reset();
      }
      if (settings.affects(event, 'probe') || settings.affects(event, 'executable')) {
        void session.check();
      }
      void refreshPlan();
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => void refreshPlan()),
  );

  updateStatusItem(session.state);
  updateWatchView();
  void refreshPlan();
  updateMonitoring();
  if (unsupported) {
    sessionTree.refresh();
  }
}

export function deactivate(): void {}
