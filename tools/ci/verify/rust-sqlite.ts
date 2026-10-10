// Prepared SQLite prefix cache: every native consumer restores and verifies the
// prefix before any build or binary use; only the producer saves it.
import {
  type Mapping,
  CACHE_PIN,
  indexOfSame,
  jobShapeErrors,
  same,
  steps,
  str,
} from "./rust-common.ts";

export const SQLITE_PREFIX_PATH =
  "${{ runner.temp }}/fvoci-sqlite/${{ steps.sqlite.outputs.target }}";
export const SQLITE_PREFIX_KEY =
  "v1-sqlite-prefix-ubuntu-26.04-${{ runner.arch }}-1.98.1-${{ steps.sqlite.outputs.cache_identity }}";
export const SQLITE_PACKAGES =
  'dpkg-query -W gcc binutils libc6 libc6-dev libclang-18-dev python3 curl > "$RUNNER_TEMP/fvoci-sqlite/build-packages.txt"';
const SQLITE_PREFLIGHT =
  'bash scripts/prepare-sqlite-ci.sh --parent "$RUNNER_TEMP/fvoci-sqlite" \\\n' +
  '  --identity-only --github-output "$GITHUB_OUTPUT"\n';
const LIBCLANG_ENV = { LIBCLANG_PATH: "/usr/lib/llvm-18/lib" };
// The producer saves; every other consumer only restores and verifies.
export const SQLITE_PREFIX_JOBS = [
  "fast",
  "native-arm64",
  "postgres-build",
  "postgres",
  "collaboration",
];
const SQLITE_PREFIX_SAVER = "postgres-build";

export function sqlitePrefixCacheSteps(saver = false): Mapping[] {
  const list: Mapping[] = [
    {
      name: "Restore prepared SQLite prefix",
      id: "sqlite_prefix_cache",
      uses: "actions/cache/restore@" + CACHE_PIN,
      with: { path: SQLITE_PREFIX_PATH, key: SQLITE_PREFIX_KEY },
    },
    {
      name: "Verify cached SQLite prefix or build",
      env: LIBCLANG_ENV,
      run:
        'bash scripts/prepare-sqlite-ci.sh --parent "$RUNNER_TEMP/fvoci-sqlite" \\\n' +
        '  --cache-fallback --expected-cache-identity "${{ steps.sqlite.outputs.cache_identity }}" \\\n' +
        '  --github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"\n',
    },
  ];
  if (saver) {
    list.push({
      name: "Save verified SQLite prefix",
      if: "steps.sqlite_prefix_cache.outputs.cache-hit != 'true'",
      uses: "actions/cache/save@" + CACHE_PIN,
      with: {
        path: SQLITE_PREFIX_PATH,
        key: "${{ steps.sqlite_prefix_cache.outputs.cache-primary-key }}",
      },
    });
  }
  return list;
}

const prefixStepNames = new Set(sqlitePrefixCacheSteps(true).map((step) => step.name));
const consumerPathPrefixes = ["target", "${{ runner.temp }}/rust-binaries"];

export function verifySqlitePrefixCache(jobs: Mapping): string[] {
  const shape = jobShapeErrors(jobs, SQLITE_PREFIX_JOBS);
  return shape.length ? shape : sqlitePrefixCacheErrors(jobs);
}

/** The prefix rules for jobs that already passed jobShapeErrors. */
export function sqlitePrefixCacheErrors(jobs: Mapping): string[] {
  const errors: string[] = [];
  for (const name of SQLITE_PREFIX_JOBS) {
    const list = steps(jobs[name]);
    const expected = sqlitePrefixCacheSteps(name === SQLITE_PREFIX_SAVER);
    const actual = list.filter(
      (step) =>
        prefixStepNames.has(step.name as string) ||
        step.id === "sqlite_prefix_cache" ||
        str(step.with, "path").includes("fvoci-sqlite"),
    );
    const prep = list.filter((step) => step.id === "sqlite");
    const only = prep[0];
    const prepKeys = only ? Object.keys(only).sort() : [];
    if (
      !same(actual, expected) ||
      prep.length !== 1 ||
      !same(prepKeys, ["env", "id", "name", "run"]) ||
      !same(only?.env, LIBCLANG_ENV) ||
      !str(only, "run").endsWith(SQLITE_PREFLIGHT) ||
      !str(only, "run").includes(SQLITE_PACKAGES)
    ) {
      errors.push(
        `rust: ${name} SQLite prefix cache must retain exact preflight/restore/verify and producer-only save`,
      );
      continue;
    }
    const order = `rust: ${name} SQLite prefix verification must precede save and all consumers`;
    const positions = [indexOfSame(list, only), ...expected.map((step) => indexOfSame(list, step))];
    if (positions.some((position, index) => index > 0 && position < (positions[index - 1] ?? 0))) {
      errors.push(order);
    }
    const verifyAt = indexOfSame(list, expected[1]);
    list.forEach((step, index) => {
      if (expected.some((item) => same(step, item)) || same(step, only)) return;
      const run = str(step, "run");
      const consumes =
        consumerPathPrefixes.some((prefix) => str(step.with, "path").startsWith(prefix)) ||
        run.includes("cargo ") ||
        run.includes("rust-binaries unpack");
      if (consumes && index < verifyAt) errors.push(order);
    });
  }
  return errors;
}
