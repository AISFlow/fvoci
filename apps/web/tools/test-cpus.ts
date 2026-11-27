/**
 * `FVOCI_TEST_CPUS`: how many processors the tests may use, said in one
 * place (`cargo xtask test` reads it too, for its test threads).
 *
 * Set, each test tool picks its workers as it would on a machine with that
 * many processors: Playwright half of them, Vitest one less. A worker count
 * a config fixes for its own reasons is lowered to that, never raised. A
 * tool's own flag (`--workers`, `--maxWorkers`) still wins. Unset, every tool
 * picks from the machine as it always did.
 */

const VARIABLE = 'FVOCI_TEST_CPUS';

/** The processors the variable gives the tests, if it is set. */
export function testCpus(env: NodeJS.ProcessEnv = process.env): number | undefined {
  const text = env[VARIABLE]?.trim();
  if (!text) return undefined;
  if (!/^\d+$/.test(text) || Number(text) < 1) {
    throw new Error(`${VARIABLE} is "${text}": give a whole number of processors, 1 or more`);
  }
  return Number(text);
}

/**
 * Playwright's workers: half the processors, as its default (`50%`) takes of
 * the machine's, and no more than `fixed` when a config fixes a number.
 * `undefined` leaves the config as it was.
 */
export function playwrightWorkers(
  fixed?: number,
  env: NodeJS.ProcessEnv = process.env,
): number | undefined {
  const cpus = testCpus(env);
  if (cpus === undefined) return fixed;
  const workers = Math.max(1, Math.floor(cpus / 2));
  return fixed === undefined ? workers : Math.min(fixed, workers);
}

/** Vitest's workers: one less than the processors, as its default of a run. */
export function vitestWorkers(env: NodeJS.ProcessEnv = process.env): number | undefined {
  const cpus = testCpus(env);
  return cpus === undefined ? undefined : Math.max(1, cpus - 1);
}
