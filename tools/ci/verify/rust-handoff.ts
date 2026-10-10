// Build-output caches and the once-per-architecture PostgreSQL binary handoff:
// the producer builds every registered executable once; consumers download that
// exact SHA/architecture/attempt artifact and never rebuild or restore caches.
import {
  type Mapping,
  type VerifyContext,
  CACHE_PIN,
  DOWNLOAD_ARTIFACT_PIN,
  POSTGRES_BUILD_SELECT_IF,
  RUST_HELPER_CACHE_KEY,
  RUST_POSTGRES_BUILD_CACHE_KEY,
  buildCacheKey,
  field,
  has,
  indexOfSame,
  isMapping,
  jobShapeErrors,
  rustJobs,
  same,
  steps,
  str,
} from "./rust-common.ts";
import {
  SQLITE_PREFIX_JOBS,
  sqlitePrefixCacheErrors,
  sqlitePrefixCacheSteps,
} from "./rust-sqlite.ts";

// [label, directory, cache identity, step id] of the fast job's qualified target caches.
const RUST_FAST_CACHES = [
  ["server", "target/default", "fast-default-test", "default_cache"],
  ["clippy", "target/clippy", "fast-db-tests-clippy", "clippy_cache"],
  ["SQLite library", "target/db-lib", "fast-db-tests-lib-test", "db_lib_cache"],
] as const;
const SAFE_CARGO_DOWNLOAD = {
  path: "~/.cargo/registry\n~/.cargo/git\n",
  key: "v2-cargo-server-ubuntu-26.04-${{ runner.arch }}-1.98.1-${{ hashFiles('Cargo.lock', 'Cargo.toml') }}",
};
const PRODUCER_STRATEGY = {
  "fail-fast": false,
  matrix: { include: [{ runner: "ubuntu-26.04" }, { runner: "ubuntu-26.04-arm" }] },
};
const PRODUCER_CACHES = [
  ["server", "target/db-tests", RUST_POSTGRES_BUILD_CACHE_KEY],
  ["schema default", "target/schema-default", buildCacheKey("schema-default-dev")],
  ["production helper", "crates/collab-engine/target", RUST_HELPER_CACHE_KEY],
] as const;
const PRODUCER_BUILD_STEP = {
  name: "Build all PostgreSQL test executables once per architecture",
  env: { CARGO_TARGET_DIR: "${{ github.workspace }}/target/db-tests" },
  run:
    'mkdir "$RUNNER_TEMP/rust-binaries"\n' +
    'python3 scripts/ci_selection.py rust-binaries build --directory "$RUNNER_TEMP/rust-binaries"\n',
};
const BINARY_CONSUMERS = [
  ["postgres", "postgres"],
  ["collaboration", "helper"],
] as const;

export function rustFastCacheSteps(): Mapping[] {
  return RUST_FAST_CACHES.flatMap(([label, directory, identity, id]) => [
    {
      name: "Restore " + label + " build outputs",
      id,
      uses: "actions/cache/restore@" + CACHE_PIN,
      with: { path: directory, key: buildCacheKey(identity) },
    },
    {
      name: "Save " + label + " build outputs after validation",
      if: `steps.${id}.outputs.cache-hit != 'true'`,
      uses: "actions/cache/save@" + CACHE_PIN,
      with: { path: directory, key: "${{ steps." + id + ".outputs.cache-primary-key }}" },
    },
  ]);
}

const isCacheStep = (step: Mapping) => str(step, "uses").startsWith("actions/cache");
const contains = (list: readonly unknown[], item: unknown) =>
  list.some((entry) => same(entry, item));

/** SQLite prefix ordering plus fast/native caches, the producer and both consumers. */
export function verifyRustBinaryHandoff(jobs: Mapping): string[] {
  const shape = jobShapeErrors(jobs, SQLITE_PREFIX_JOBS);
  if (shape.length) return shape;
  const errors = sqlitePrefixCacheErrors(jobs);
  const require = (condition: boolean, message: string) => {
    if (!condition) errors.push("rust: " + message);
  };
  const consumerPrefix = sqlitePrefixCacheSteps();
  const fastSteps = steps(jobs.fast);
  const expected = rustFastCacheSteps();
  const expectedNames = new Set(expected.map((step) => step.name));
  const actual = fastSteps.filter((step) => expectedNames.has(step.name as string));
  require(actual.length === expected.length &&
    expected.every((step) =>
      contains(actual, step),
    ), "fast server cache must retain exact pinned restore/save pairs and complete inputs");
  for (const step of fastSteps) {
    if (isCacheStep(step) && !contains(expected, step) && !contains(consumerPrefix, step)) {
      require(Object.keys(step).every((key) => ["name", "uses", "with"].includes(key)) &&
        same(
          field(step, "with"),
          SAFE_CARGO_DOWNLOAD,
        ), "only the exact qualified target restore/save and safe Cargo downloads are allowed");
    }
  }
  const lastTest = fastSteps.reduce(
    (last, step, index) => (str(step, "run").includes("cargo test") ? index : last),
    -1,
  );
  for (const save of expected.filter((_, index) => index % 2 === 1)) {
    if (contains(fastSteps, save)) {
      require(indexOfSame(fastSteps, save) >
        lastTest, "fast cache saves must follow all validation");
    }
  }
  const nativeCache = steps(jobs["native-arm64"]).filter(
    (step) => step.name === "Restore server build outputs",
  );
  require(nativeCache.length === 1 &&
    same(field(nativeCache[0], "with"), {
      path: "target",
      key: buildCacheKey("native-default-dev-test"),
    }), "native default-feature cache must be distinct from DB test outputs");
  const producer = isMapping(jobs["postgres-build"]) ? jobs["postgres-build"] : {};
  require(producer.needs === "ci-plan" &&
    producer.if === POSTGRES_BUILD_SELECT_IF, "binary producer must use registered selection");
  require(producer["runs-on"] === "${{ matrix.runner }}" &&
    same(
      field(producer, "strategy"),
      PRODUCER_STRATEGY,
    ), "binary producer must run once per architecture");
  require(!has(producer, "services") &&
    !has(producer, "env") &&
    !has(
      producer,
      "continue-on-error",
    ), "binary producer must remain credential-free and fail closed");
  const producerSteps = steps(producer);
  for (const [label, path, key] of PRODUCER_CACHES) {
    const name = "Restore " + label + " build outputs";
    require(same(
      producerSteps.filter((step) => step.name === name),
      [{ name, uses: "actions/cache@" + CACHE_PIN, with: { path, key } }],
    ), "producer cache must have one exact writer and complete inputs");
  }
  require(same(
    producerSteps.filter((step) => step.name === PRODUCER_BUILD_STEP.name),
    [PRODUCER_BUILD_STEP],
  ), "binary producer must build the complete registered cohort");
  for (const [jobName, cohort] of BINARY_CONSUMERS) {
    const job = jobs[jobName];
    require(same(field(job, "needs"), [
      "ci-plan",
      "postgres-build",
    ]), "binary consumers must need the successful producer");
    const list = steps(job);
    const downloadName = `Download required ${cohort} executables for this SHA and architecture`;
    const download = list.filter((step) => step.name === downloadName);
    require(same(download, [
      {
        name: downloadName,
        uses: DOWNLOAD_ARTIFACT_PIN,
        with: {
          name:
            "rust-" + cohort + "-${{ runner.arch }}-${{ github.sha }}-${{ github.run_attempt }}",
          path: "${{ runner.temp }}/rust-binaries",
        },
      },
    ]), "binary consumers must download the exact SHA/architecture/attempt artifact");
    const validateName = `Validate and restore finished ${cohort} executables (no rebuild fallback)`;
    const validate = list.filter((step) => step.name === validateName);
    require(same(validate, [
      {
        name: validateName,
        run:
          "python3 scripts/ci_selection.py rust-binaries unpack --cohort " +
          cohort +
          ' --directory "$RUNNER_TEMP/rust-binaries" --sqlite-identity "${{ steps.sqlite.outputs.cache_identity }}"',
      },
    ]), "binary consumers must validate all inputs and hashes unconditionally");
    if (download.length && validate.length) {
      require(indexOfSame(list, download[0]) <
        indexOfSame(list, validate[0]), "download must precede binary validation");
    }
    if (jobName === "postgres") {
      require(!list.some(
        (step) =>
          ["cargo ", '"cargo"', "'cargo'"].some((word) => str(step, "run").includes(word)) ||
          (isCacheStep(step) && !contains(consumerPrefix, step)),
      ), "PostgreSQL consumers must never restore build caches or rebuild");
    } else {
      require(!list.some(
        (step) =>
          str(step, "run").includes("--bin collab-engine") ||
          field(step.with, "path") === "crates/collab-engine/target",
      ), "collaboration must reuse helper without build/cache fallback");
    }
  }
  return errors;
}

export function verifyRustBinaryHandoffCtx(ctx: VerifyContext): string[] {
  const jobs = rustJobs(ctx);
  return jobs ? verifyRustBinaryHandoff(jobs) : [];
}
