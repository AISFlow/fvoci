// Process helpers for the web e2e harness. External commands stay argv arrays
// so PATH stubs and the unchanged shell callees see the same invocation.

import { spawn } from "node:child_process";

export class Fail extends Error {
  constructor(
    message: string,
    readonly code = 1,
  ) {
    super(message);
    this.name = "Fail";
  }
}

export type CommandResult = {
  status: number;
  stdout: string;
  stderr: string;
};

export type CommandOptions = {
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  stdin?: "inherit" | "ignore";
  stdout?: "inherit" | "pipe" | "ignore";
  stderr?: "inherit" | "pipe" | "ignore";
};

function statusOf(code: number | null, signal: NodeJS.Signals | null): number {
  if (code !== null) return code;
  if (signal) return 1;
  return 1;
}

export function command(args: string[], options: CommandOptions = {}): Promise<CommandResult> {
  const stdoutMode = options.stdout ?? "pipe";
  const stderrMode = options.stderr ?? "pipe";
  return new Promise((resolve, reject) => {
    const child = spawn(args[0] ?? "", args.slice(1), {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: [options.stdin ?? "ignore", stdoutMode, stderrMode],
    });
    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    child.stdout?.on("data", (chunk: Buffer) => stdout.push(chunk));
    child.stderr?.on("data", (chunk: Buffer) => stderr.push(chunk));
    child.on("error", reject);
    child.on("close", (code, signal) => {
      resolve({
        status: statusOf(code, signal),
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
      });
    });
  });
}

export async function commandStatus(args: string[], options: CommandOptions = {}): Promise<number> {
  const result = await command(args, {
    ...options,
    stdout: options.stdout ?? "inherit",
    stderr: options.stderr ?? "inherit",
  });
  return result.status;
}

export async function commandText(args: string[], options: CommandOptions = {}): Promise<string> {
  const result = await command(args, options);
  if (result.status !== 0) {
    throw new Fail(result.stderr.trim() || `${args[0]} exited ${result.status}`, result.status);
  }
  return result.stdout;
}

export function quoteExport(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}
