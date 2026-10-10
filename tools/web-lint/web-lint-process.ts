import { spawnSync } from "node:child_process";
import { constants } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export type SpawnResult = {
  status: number | null;
  signal: NodeJS.Signals | null;
  error?: NodeJS.ErrnoException;
};

export type SpawnFn = (command: readonly string[], cwd: string) => SpawnResult;

export function repositoryRoot(): string {
  return resolve(dirname(fileURLToPath(import.meta.url)), "../..");
}

export function withDefaults(argv: readonly string[], defaults: readonly string[]): string[] {
  if (argv.length === 0 || argv[0].startsWith("-")) return [...defaults, ...argv];
  return [...argv];
}

export function exitCodeOf(result: SpawnResult): number {
  if (result.error) {
    if (result.error.code === "ENOENT") return 127;
    if (result.error.code === "EACCES") return 126;
    return 1;
  }
  if (result.signal) {
    const number = constants.signals[result.signal];
    return 128 + (typeof number === "number" ? number : 1);
  }
  return result.status ?? 1;
}

export function spawnInherit(command: readonly string[], cwd: string): SpawnResult {
  const result = spawnSync(command[0], command.slice(1), { cwd, stdio: "inherit" });
  return {
    status: result.status,
    signal: result.signal,
    error: result.error as NodeJS.ErrnoException | undefined,
  };
}

export function runChecked(
  commands: readonly (readonly string[])[],
  cwd: string,
  spawn: SpawnFn = spawnInherit,
): number {
  for (const command of commands) {
    const code = exitCodeOf(spawn(command, cwd));
    if (code !== 0) return code;
  }
  return 0;
}
