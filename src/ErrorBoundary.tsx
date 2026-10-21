import { Component, type ReactNode } from "react";

interface State {
  error: Error | null;
}

/** Shows a render error in place of the app instead of an empty window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <main className="flex h-full items-center justify-center bg-surface p-8">
        <div className="max-w-3xl">
          <h1 className="text-[18px] font-semibold text-danger">The app hit an error and stopped drawing</h1>
          <p className="mt-2 text-muted">Reload the window to start again. The details below help fix it.</p>
          <pre className="mt-4 max-h-96 overflow-auto rounded-sm border border-rule bg-panel p-3 font-mono text-[12px] whitespace-pre-wrap">
            {error.stack ?? String(error)}
          </pre>
          <button
            onClick={() => location.reload()}
            className="mt-4 rounded-sm border border-rule bg-panel px-3 py-1 font-medium hover:bg-sunken"
          >
            Reload
          </button>
        </div>
      </main>
    );
  }
}
