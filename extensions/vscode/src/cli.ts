import { execFile } from 'node:child_process';
import * as vscode from 'vscode';
import { classifyFailure, Failure } from './cliText';
import { executable } from './settings';

export type CliResult =
  | { ok: true; stdout: string; stderr: string }
  | { ok: false; failure: Failure; stdout: string; stderr: string };

export interface RunOptions {
  timeoutMs: number;
  /** Log the command and its result; polls pass false to keep the log readable. */
  log?: boolean;
}

/**
 * Runs tuning-studio commands one at a time, without a shell. Every call goes
 * through one queue, so a poll, a set and a tasks sample never overlap.
 */
export class Cli {
  private tail: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly log: vscode.OutputChannel,
    private readonly cwd: () => string | undefined,
  ) {}

  run(args: string[], options: RunOptions): Promise<CliResult> {
    const next = this.tail.then(() => this.exec(args, options));
    this.tail = next.catch(() => undefined);
    return next;
  }

  private exec(args: string[], options: RunOptions): Promise<CliResult> {
    const command = executable();
    if (options.log) {
      this.log.appendLine(`$ ${[command, ...args].map(quote).join(' ')}`);
    }
    return new Promise((resolve) => {
      execFile(
        command,
        args,
        {
          cwd: this.cwd(),
          timeout: options.timeoutMs,
          maxBuffer: 16 * 1024 * 1024,
          windowsHide: true,
          encoding: 'utf8',
        },
        (error, stdout, stderr) => {
          if (!error) {
            if (options.log && stdout.trim()) {
              this.log.appendLine(stdout.trimEnd());
            }
            resolve({ ok: true, stdout, stderr });
            return;
          }
          const failure = classifyFailure({
            stderr,
            code: (error as NodeJS.ErrnoException).code,
            killed: error.killed,
            executable: command,
          });
          if (options.log) {
            this.log.appendLine(`  failed: ${failure.message}`);
          }
          resolve({ ok: false, failure, stdout, stderr });
        },
      );
    });
  }
}

function quote(word: string): string {
  return /^[\w@%+=:,./-]+$/.test(word) ? word : `'${word.replace(/'/g, `'\\''`)}'`;
}
