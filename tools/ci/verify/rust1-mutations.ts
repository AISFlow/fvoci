// rust.yml mutation corpus for the rust1 tests; an out-of-tree differential also
// replays it against scripts/ci_selection.py verify-workflows.
// Each entry edits a parsed copy and names a message its check must report.
import { YAML } from "bun";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { POSTGRES_MATRIX_CATALOG_ENV } from "./rust-postgres.ts";
import { RUST_DEFAULT_LIBRARY_STEP, RUST_SELECTED_LIBRARY_STEP } from "./rust-library.ts";
import { SQLITE_PACKAGES, SQLITE_PREFIX_JOBS, sqlitePrefixCacheSteps } from "./rust-sqlite.ts";
import { same } from "./rust-common.ts";

/* eslint-disable @typescript-eslint/no-explicit-any, @typescript-eslint/no-unsafe-assignment, @typescript-eslint/no-unsafe-member-access, @typescript-eslint/no-unsafe-return, @typescript-eslint/no-unsafe-argument, @typescript-eslint/no-unsafe-call, @typescript-eslint/restrict-plus-operands -- mutations edit untyped parsed YAML in place */
type Doc = any;
export type Mutation = {
  name: string;
  mutate: (workflow: Doc) => void;
  needle: string;
  /** Python raises a traceback here instead of reporting; the differential skips it. */
  pythonRaises?: boolean;
};

export function loadRustWorkflow(root: string): Doc {
  return YAML.parse(readFileSync(join(root, ".github/workflows/rust.yml"), "utf8"));
}

const named = (list: Doc[], name: string) => {
  const found = list.find((step) => step?.name === name);
  if (!found) throw new Error(`fixture step ${name} missing`);
  return found;
};
const catalog = (workflow: Doc): Doc[] =>
  JSON.parse(workflow.jobs.postgres.env[POSTGRES_MATRIX_CATALOG_ENV]);
const storeCatalog = (workflow: Doc, rows: unknown) => {
  workflow.jobs.postgres.env[POSTGRES_MATRIX_CATALOG_ENV] = JSON.stringify(rows);
};
const editCatalog = (edit: (rows: Doc[], a16: Doc, c16: Doc) => void) => (workflow: Doc) => {
  const rows = catalog(workflow);
  const c16 = rows.find((row) => row.shard === "c" && row.pg_major === "16");
  const a16 = rows.find((row) => row.shard === "a" && row.pg_major === "16");
  edit(rows, a16, c16);
  storeCatalog(workflow, rows);
};
const libraryStep = (workflow: Doc) => named(workflow.jobs.fast.steps, RUST_SELECTED_LIBRARY_STEP);
const editLibraryRun = (from: string, to: string) => (workflow: Doc) => {
  const step = libraryStep(workflow);
  if (!step.run.includes(from)) throw new Error(`fixture text ${from} missing`);
  step.run = step.run.replace(from, () => to);
};
const producerServerCache = (workflow: Doc) =>
  named(workflow.jobs["postgres-build"].steps, "Restore server build outputs");
const fastServerCache = (workflow: Doc) =>
  named(workflow.jobs.fast.steps, "Restore server build outputs");
const BUDGET = "rust: PostgreSQL budget must retain A/C15m, x64 B20m and ARM64 B25m";
const LIBRARY = "selected library";
const STRICT_CACHE = "rust: PostgreSQL producer must retain strict complete-input server cache";
const FAST_CACHE = "fast server cache must retain exact pinned restore/save";
const EXTRA_CACHE = "only the exact qualified target restore/save and safe Cargo downloads";

const library: Mutation[] = (
  [
    ["missing", (w: Doc) => w.jobs.fast.steps.splice(w.jobs.fast.steps.indexOf(libraryStep(w)), 1)],
    ["duplicate", (w: Doc) => w.jobs.fast.steps.push({ ...libraryStep(w) })],
    ["skip", (w: Doc) => (libraryStep(w).if = "false")],
    ["masked", (w: Doc) => (libraryStep(w)["continue-on-error"] = true)],
    ["env", (w: Doc) => (libraryStep(w).env = { RUST_TEST_THREADS: "0" })],
    ["job-env", (w: Doc) => (w.jobs.fast.env = { FVOCI_DATABASE_BACKEND: "libsql-remote" })],
    ["wrong-feature", editLibraryRun('"db-tests"', '"api-schema"')],
    ["no-exact", editLibraryRun(', "--exact"', "")],
    ["no-lib", editLibraryRun('"--lib", ', "")],
    [
      "missing-loop-filter",
      editLibraryRun("in RUST_SELECTED_LIBRARY_FILTERS:", "in RUST_SELECTED_LIBRARY_FILTERS[:-1]:"),
    ],
    ["swallow-command", (w: Doc) => (libraryStep(w).run += "true\n")],
    [
      "missing-default",
      (w: Doc) => {
        const steps: Doc[] = w.jobs.fast.steps;
        steps.splice(
          steps.findIndex((step) => same(step, RUST_DEFAULT_LIBRARY_STEP)),
          1,
        );
      },
    ],
    [
      "default-after-selected",
      (w: Doc) => {
        const steps: Doc[] = w.jobs.fast.steps;
        const [plain] = steps.splice(
          steps.findIndex((step) => same(step, RUST_DEFAULT_LIBRARY_STEP)),
          1,
        );
        steps.push(plain);
      },
    ],
  ] as [string, (workflow: Doc) => void][]
).map(([name, mutate]) => ({ name: "library:" + name, mutate, needle: LIBRARY }));

const budgetRows: Mutation[] = (
  [
    ["missing-c", (rows, _a, c) => rows.splice(rows.indexOf(c), 1), "all twelve"],
    ["duplicate-row", (rows, _a, c) => rows.push({ ...c }), "duplicate matrix row"],
    ["unknown-shard", (_r, _a, c) => (c.shard = "d"), "unsupported platform/major/shard"],
    ["wrong-major", (_r, _a, c) => (c.pg_major = "17"), "duplicate matrix row"],
    ["wrong-pin", (_r, _a, c) => (c.postgres_image = "postgres:16"), "signed image pins"],
    ["duplicate-check", (_r, a, c) => (c.check = a.check), "check names"],
    [
      "duplicate-target",
      (_r, _a, c) => (c.tests += " --test task_integration"),
      "duplicates a target",
    ],
    [
      "duplicate-across-shards",
      (_r, a) => (a.tests += " --test task_integration"),
      "duplicates a target",
    ],
    [
      "missing-pg16-target",
      (_r, a) => (a.tests = a.tests.replace("--test db_integration ", "")),
      "equal target coverage",
    ],
    ["c-filter", (_r, _a, c) => (c.tests += " -- --skip failing"), "complete --test target pairs"],
    ["extra-row-field", (_r, _a, c) => (c["continue-on-error"] = true), "exact execution fields"],
    ["shard-number", (_r, _a, c) => (c.shard = 3), "unsupported platform/major/shard"],
    [
      "c-extra-target",
      (_r, a, c) => {
        const moved = a.tests.split(" ").slice(-2).join(" ");
        a.tests = a.tests.split(" ").slice(0, -2).join(" ");
        c.tests += " " + moved;
      },
      "C must run exactly task/comment",
    ],
  ] as [string, (rows: Doc[], a16: Doc, c16: Doc) => void, string][]
).map(([name, edit, needle]) => ({
  name: "budget-row:" + name,
  mutate: editCatalog(edit),
  needle,
}));

const budgetLimits: Mutation[] = (
  [
    ["timeout", (w) => (w.jobs.postgres["timeout-minutes"] = 20), BUDGET],
    ["timeout-missing", (w) => delete w.jobs.postgres["timeout-minutes"], BUDGET],
    [
      "timeout-arm-shard",
      (w) => (w.jobs.postgres["timeout-minutes"] = "${{ matrix.shard == 'b' && 25 || 15 }}"),
      BUDGET,
    ],
    [
      "fail-fast",
      (w) => (w.jobs.postgres.strategy["fail-fast"] = true),
      "every selected matrix row",
    ],
    ["mask", (w) => (w.jobs.postgres["continue-on-error"] = true), "without error masking"],
    ["runner", (w) => (w.jobs.postgres["runs-on"] = "ubuntu-26.04"), "without error masking"],
    [
      "extra-dimension",
      (w) => (w.jobs.postgres.strategy.matrix = { include: catalog(w), exclude: [] }),
      "without an empty-array fallback",
    ],
    ["strategy-scalar", (w) => (w.jobs.postgres.strategy = "matrix"), "every selected matrix row"],
    [
      "cache-fallback",
      (w) => (producerServerCache(w).with["restore-keys"] = "v2-server-"),
      STRICT_CACHE,
    ],
    [
      "cache-key",
      (w) => (producerServerCache(w).with.key = "v2-server-ubuntu-26.04-incomplete"),
      STRICT_CACHE,
    ],
    ["cache-path", (w) => (producerServerCache(w).with.path = "/foreign/target"), STRICT_CACHE],
    [
      "missing-cache",
      (w) =>
        w.jobs["postgres-build"].steps.splice(
          w.jobs["postgres-build"].steps.indexOf(producerServerCache(w)),
          1,
        ),
      STRICT_CACHE,
    ],
    [
      "catalog-missing",
      (w) =>
        (w.jobs.postgres.env = {
          ...w.jobs.postgres.env,
          [POSTGRES_MATRIX_CATALOG_ENV]: undefined,
        }),
      "catalog missing",
    ],
    [
      "catalog-not-json",
      (w) => (w.jobs.postgres.env[POSTGRES_MATRIX_CATALOG_ENV] = "[{"),
      "catalog is not JSON",
    ],
    [
      "catalog-empty",
      (w) => (w.jobs.postgres.env[POSTGRES_MATRIX_CATALOG_ENV] = "[]"),
      "non-empty list",
    ],
    [
      "catalog-row-scalar",
      (w) => {
        storeCatalog(w, [...catalog(w), 7]);
      },
      "row must be a mapping",
    ],
    [
      "no-run-fragment",
      editCatalog((rows) => (rows[0].tests = "--no-run --test db_integration")),
      "--no-run",
    ],
    [
      "runner-unknown",
      editCatalog((rows) => (rows[0].runner = "ubuntu-22.04")),
      "unknown runner 'ubuntu-22.04'",
    ],
    ["runner-null", editCatalog((rows) => (rows[0].runner = null)), "unknown runner None"],
    ["runner-number", editCatalog((rows) => (rows[0].runner = 16)), "unknown runner 16"],
    [
      "runner-nonprintable",
      // YAML folds NEL/LS/PS line breaks before Python sees them, so use Cf/Zs/Co.
      editCatalog((rows) => (rows[0].runner = "ubuntu\xad ​　é\"'")),
      "unknown runner 'ubuntu\\xad \\u200b\\u3000\\ue000é\"\\''",
    ],
    [
      "tests-missing",
      editCatalog((rows) => delete rows[0].tests),
      "missing tests command fragment",
    ],
    ["tests-pipe", editCatalog((rows) => (rows[0].tests += " | true")), "shell operator '|'"],
    [
      "arm-drops-target",
      editCatalog((rows) => {
        for (const row of rows) {
          if (row.runner === "ubuntu-26.04-arm")
            row.tests = row.tests.replace(" --test search_meili", "");
        }
      }),
      "equal target coverage",
    ],
  ] as [string, (workflow: Doc) => void, string][]
).map(([name, mutate, needle]) => ({ name: "budget:" + name, mutate, needle }));

const sqliteKinds: [
  string,
  (steps: Doc[], restore: Doc, verify: Doc, prep: Doc) => void,
  string,
][] = [
  ["pin", (_s, restore) => (restore.uses = "actions/cache/restore@v4"), "exact preflight"],
  ["key", (_s, restore) => (restore.with.key = "unqualified"), "exact preflight"],
  ["path", (_s, restore) => (restore.with.path = "target"), "exact preflight"],
  [
    "restore-fallback",
    (_s, restore) => (restore.with["restore-keys"] = "v1-sqlite"),
    "exact preflight",
  ],
  [
    "save-consumer",
    (steps) => steps.push(structuredClone(sqlitePrefixCacheSteps(true)[2])),
    "exact preflight",
  ],
  [
    "skip-verify",
    (_s, _r, verify) => (verify.if = "steps.sqlite_prefix_cache.outputs.cache-hit != 'true'"),
    "exact preflight",
  ],
  [
    "missing-verify",
    (steps, _r, verify) => steps.splice(steps.indexOf(verify), 1),
    "exact preflight",
  ],
  [
    "reorder",
    (steps, _r, verify) => steps.unshift(...steps.splice(steps.indexOf(verify), 1)),
    "exact preflight",
  ],
  [
    "expected-identity",
    (_s, _r, verify) =>
      (verify.run = verify.run.replace("--expected-cache-identity", "--foreign-identity")),
    "exact preflight",
  ],
  ["skip-preflight", (_s, _r, _v, prep) => (prep["continue-on-error"] = true), "exact preflight"],
  [
    "packages",
    (_s, _r, _v, prep) => (prep.run = prep.run.replace(SQLITE_PACKAGES, "dpkg-query -W")),
    "exact preflight",
  ],
  [
    "early-consumer",
    (steps) => steps.unshift({ run: "cargo build --locked" }),
    "must precede save and all consumers",
  ],
  [
    "early-binary-path",
    (steps) =>
      steps.unshift({
        uses: "actions/download-artifact@v4",
        with: { path: "${{ runner.temp }}/rust-binaries" },
      }),
    "must precede save and all consumers",
  ],
];
const sqlite: Mutation[] = SQLITE_PREFIX_JOBS.flatMap((job) =>
  sqliteKinds.map(([kind, edit, needle]) => ({
    name: `sqlite:${job}:${kind}`,
    needle: `rust: ${job} SQLite prefix ${needle === "exact preflight" ? "cache must retain exact preflight" : "verification " + needle}`,
    mutate: (w: Doc) => {
      const steps: Doc[] = w.jobs[job].steps;
      edit(
        steps,
        named(steps, "Restore prepared SQLite prefix"),
        named(steps, "Verify cached SQLite prefix or build"),
        steps.find((step) => step.id === "sqlite"),
      );
    },
  })),
);

const fastCache: Mutation[] = (
  [
    ["combined-post", (c) => (c.uses = c.uses.replace("cache/restore@", "cache@"))],
    ["wrong-pin", (c) => (c.uses = "actions/cache/restore@v4")],
    ["path", (c) => (c.with.path = "/foreign/target")],
    ["architecture", (c) => (c.with.key = c.with.key.replace("${{ runner.arch }}", "fixed"))],
    ["toolchain", (c) => (c.with.key = c.with.key.replace("1.98.1", "old"))],
    [
      "sqlite",
      (c) => (c.with.key = c.with.key.replace("${{ steps.sqlite.outputs.cache_identity }}", "")),
    ],
    ["source", (c) => (c.with.key = c.with.key.replace("'src/**', ", ""))],
    ["fallback", (c) => (c.with["restore-keys"] = "v2-server-")],
    ["skip", (c) => (c.if = "false")],
  ] as [string, (cache: Doc) => void][]
).map(([name, edit]) => ({
  name: "fast-cache:" + name,
  mutate: (w: Doc) => {
    edit(fastServerCache(w));
  },
  needle: FAST_CACHE,
}));

const extraCache = (family: string, path: string, rename: string, key?: string) => (w: Doc) => {
  const step = structuredClone(fastServerCache(w));
  step.name = rename;
  step.uses = step.uses.replace(
    "cache/restore@",
    family === "combined" ? "cache@" : "cache/" + family + "@",
  );
  step.with.path = path;
  if (key) step.with.key = key;
  w.jobs.fast.steps.push(step);
};
const extraCaches: Mutation[] = [
  ...["save", "restore", "combined"].flatMap((family) =>
    [
      "tar?et/**",
      "tar*/**",
      "**/target",
      "${{ github.workspace }}/**",
      "~/.cargo/registry\n~/.cargo/git\n../target",
    ].map((path) => ({
      name: `extra-cache:${family}:${path}`,
      mutate: extraCache(family, path, "Extra cache with uncertain qualification"),
      needle: EXTRA_CACHE,
    })),
  ),
  {
    name: "extra-cache:restore:target",
    mutate: extraCache(
      "restore",
      "target",
      "Extra cache with uncertain qualification",
      "v2-server-ubuntu-26.04-${{ runner.arch }}-1.98.1-unqualified",
    ),
    needle: EXTRA_CACHE,
  },
  ...["save", "combined"].flatMap((family) =>
    [
      "target",
      "./target",
      "target/debug",
      "${{ github.workspace }}/target",
      ".",
      "~/.cargo/registry\ntarget",
    ].map((path) => ({
      name: `renamed-writer:${family}:${path}`,
      mutate: extraCache(family, path, "Differently named optional save"),
      needle: EXTRA_CACHE,
    })),
  ),
];

const handoff: Mutation[] = (
  [
    [
      "producer-needs",
      (w) => (w.jobs["postgres-build"].needs = ["ci-plan"]),
      "registered selection",
    ],
    ["producer-if", (w) => (w.jobs["postgres-build"].if = "true"), "registered selection"],
    [
      "producer-single-arch",
      (w) => w.jobs["postgres-build"].strategy.matrix.include.pop(),
      "once per architecture",
    ],
    [
      "producer-env",
      (w) => (w.jobs["postgres-build"].env = { DATABASE_URL: "x" }),
      "credential-free",
    ],
    ["producer-services", (w) => (w.jobs["postgres-build"].services = {}), "credential-free"],
    [
      "producer-helper-cache-key",
      (w) =>
        (named(w.jobs["postgres-build"].steps, "Restore production helper build outputs").with.key =
          "x"),
      "one exact writer",
    ],
    [
      "producer-build-run",
      (w) =>
        (named(
          w.jobs["postgres-build"].steps,
          "Build all PostgreSQL test executables once per architecture",
        ).run += "true\n"),
      "complete registered cohort",
    ],
    [
      "consumer-needs",
      (w) => (w.jobs.postgres.needs = ["ci-plan"]),
      "need the successful producer",
    ],
    [
      "consumer-download-attempt",
      (w) =>
        (named(
          w.jobs.postgres.steps,
          "Download required postgres executables for this SHA and architecture",
        ).with.name = "rust-postgres-${{ runner.arch }}-${{ github.sha }}"),
      "exact SHA/architecture/attempt",
    ],
    [
      "consumer-validate-order",
      (w) => {
        const steps: Doc[] = w.jobs.collaboration.steps;
        const validate = named(
          steps,
          "Validate and restore finished helper executables (no rebuild fallback)",
        );
        steps.splice(steps.indexOf(validate), 1);
        steps.splice(
          steps.indexOf(
            named(steps, "Download required helper executables for this SHA and architecture"),
          ),
          0,
          validate,
        );
      },
      "download must precede binary validation",
    ],
    [
      "consumer-validate-skip",
      (w) =>
        (named(
          w.jobs.postgres.steps,
          "Validate and restore finished postgres executables (no rebuild fallback)",
        ).if = "false"),
      "validate all inputs and hashes",
    ],
    [
      "postgres-rebuild",
      (w) => w.jobs.postgres.steps.push({ run: "cargo build --tests" }),
      "never restore build caches or rebuild",
    ],
    [
      "postgres-cache",
      (w) =>
        w.jobs.postgres.steps.push({
          uses: "actions/cache/restore@v4",
          with: { path: "x", key: "y" },
        }),
      "never restore build caches or rebuild",
    ],
    [
      "collaboration-helper-build",
      (w) => w.jobs.collaboration.steps.push({ run: "cargo build --bin collab-engine" }),
      "reuse helper without build/cache fallback",
    ],
    [
      "native-cache-shared",
      (w) =>
        (named(w.jobs["native-arm64"].steps, "Restore server build outputs").with.path =
          "target/db-tests"),
      "native default-feature cache",
    ],
    [
      "fast-save-before-test",
      (w) => {
        const steps: Doc[] = w.jobs.fast.steps;
        const save = named(steps, "Save server build outputs after validation");
        steps.splice(steps.indexOf(save), 1);
        steps.splice(
          steps.findIndex((step) => String(step.run ?? "").includes("cargo test")),
          0,
          save,
        );
      },
      "saves must follow all validation",
    ],
    [
      "fast-steps-scalar",
      (w) => w.jobs.fast.steps.push("cargo test"),
      "job must be a mapping",
      true,
    ],
    [
      "postgres-run-null",
      (w) => w.jobs.postgres.steps.push({ run: null }),
      "job must be a mapping",
      true,
    ],
  ] as [string, (workflow: Doc) => void, string, boolean?][]
).map(([name, mutate, needle, pythonRaises]) => ({
  name: "handoff:" + name,
  mutate,
  needle,
  pythonRaises,
}));

export const RUST1_MUTATIONS: readonly Mutation[] = [
  ...library,
  ...budgetRows,
  ...budgetLimits,
  ...sqlite,
  ...fastCache,
  ...extraCaches,
  ...handoff,
];
