import { expect, test } from "bun:test";
import { appendFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { pyRepr, pySplitlines, pyStrip, type Mapping } from "./py.ts";
import {
  collaborationScriptInventory,
  collaborationScriptInventoryFromText,
} from "./rust-collab.ts";
import { postgresS3Inventory, verifyPostgresIntegrationExecution } from "./rust-exec.ts";
import { selectedInstallInventory } from "./rust-install.ts";
import { verifyNativeArm64Execution } from "./rust-native.ts";
import { verifyRustSuiteRegistry } from "./rust-registry.ts";
import { RUST_SCHEMA_BASELINE_STEP, schemaBaselineInventory } from "./rust-schema.ts";
import {
  RUST_COLLAB_CI_SCRIPT,
  RUST_COLLAB_INTEGRATION_STEP,
  RUST_NATIVE_ARM64_RUN,
  RUST_POSTGRES_INTEGRATION_STEP,
  RUST_S3_INTEGRATION_STEP,
  RUST_SELECTED_INSTALL_STEP,
  cargoTestFlagsInText,
  validateMatrixTestsFragment,
} from "./rust-shared.ts";
import {
  ROOT,
  RegistryTree,
  catalogRows,
  loadContext,
  namedStep,
  realJobs,
  storeCatalog,
} from "./rust2-fixture.ts";

const clone = <T>(value: T): T => structuredClone(value);
const joined = (errors: string[] | string | null) =>
  Array.isArray(errors) ? errors.join("\n") : (errors ?? "");

function registryErrors(extra: string[], mutate?: (jobs: Mapping) => void): string[] {
  using tree = new RegistryTree(extra);
  return verifyRustSuiteRegistry(tree.context(mutate));
}

test("python value helpers keep the original string semantics", () => {
  expect(pyRepr("matrix.shard == 'b'")).toBe(`"matrix.shard == 'b'"`);
  expect(pyRepr("a'b\"c")).toBe(`'a\\'b"c'`);
  expect(pyRepr(null)).toBe("None");
  expect(pyRepr([true, 1, { k: "v" }])).toBe("[True, 1, {'k': 'v'}]");
  expect(pySplitlines("a\n\nb\r\n")).toEqual(["a", "", "b"]);
  expect(pySplitlines("")).toEqual([]);
  expect(pyStrip("\x1c x \x85")).toBe("x");
});

test("rust suite inventory matches the repository", () => {
  expect(verifyRustSuiteRegistry(loadContext(ROOT))).toEqual([]);
});

test("trimmed inventory with the real workflow passes", () => {
  expect(registryErrors([])).toEqual([]);
});

test("schema baseline target mapping refuses omissions and masking", () => {
  const jobs = realJobs("rust.yml");
  expect(schemaBaselineInventory(jobs)).toEqual([new Set(["schema_baseline_integration"]), null]);
  const mutations: Record<string, (step: Mapping, bad: Mapping) => void> = {
    missing: (step, bad) => {
      const steps = (bad["postgres"] as Mapping)["steps"] as Mapping[];
      steps.splice(steps.indexOf(step), 1);
    },
    masked: (step) => void (step["continue-on-error"] = true),
    "wrong-row": (step) => void (step["if"] = "matrix.shard == 'b'"),
    "missing-url": (step) => {
      delete (step["env"] as Mapping)["PREPARATION_DATABASE_URL"];
    },
    skip: (step) =>
      void (step["run"] = (step["run"] as string).replace(
        "'--nocapture'",
        "'--skip','postgres_catalog_dump'",
      )),
    count: (step) =>
      void (step["run"] = (step["run"] as string).replace(
        "4 passed; 0 failed; 0 ignored;",
        "3 passed; 0 failed; 0 ignored;",
      )),
    "owner-in-tests": (step) =>
      void (step["run"] = (step["run"] as string).replace(
        "'DATABASE_URL':owner_url",
        "'TEST_DATABASE_URL':owner_url",
      )),
  };
  for (const [name, mutate] of Object.entries(mutations)) {
    const bad = clone(jobs);
    mutate(namedStep(bad["postgres"] as Mapping, RUST_SCHEMA_BASELINE_STEP), bad);
    expect(bad, name).not.toEqual(jobs);
    const [names, err] = schemaBaselineInventory(bad);
    expect(names, name).toBeNull();
    expect(err, name).not.toBeNull();
  }
  for (const [runner, major] of [
    ["ubuntu-26.04", "16"],
    ["ubuntu-26.04-arm", "18"],
  ]) {
    const bad = clone(jobs);
    const job = bad["postgres"] as Mapping;
    storeCatalog(
      job,
      catalogRows(job).filter(
        (row) => !(row["runner"] === runner && row["pg_major"] === major && row["shard"] === "a"),
      ),
    );
    expect(schemaBaselineInventory(bad)[1]).toBe(
      "rust: schema baseline requires PG16/17/18 x64 and PG18 arm64 A execution",
    );
  }
});

test("schema target omission is a registry failure", () => {
  const errors = registryErrors(["schema_baseline_integration"], (jobs) => {
    const job = jobs["postgres"] as Mapping;
    job["steps"] = (job["steps"] as Mapping[]).filter(
      (step) => step["name"] !== RUST_SCHEMA_BASELINE_STEP,
    );
  });
  expect(joined(errors)).toContain(RUST_SCHEMA_BASELINE_STEP);
});

test("native arm64 wrong scheduler and check weakening fail", () => {
  const cases: [string, unknown, string][] = [
    ["runs-on", "ubuntu-26.04", "run once on ubuntu-26.04-arm"],
    ["strategy", { matrix: { shard: ["a", "b"] } }, "run once"],
    ["timeout-minutes", 20, "15 minute budget"],
    ["continue-on-error", true, "fail on build/policy errors"],
    ["if", "false", "exact unconditional"],
    ["env", { CARGO_TARGET_DIR: "other" }, "exact unconditional"],
    ["run", "cargo test --locked --offline --lib --features db-tests", "exact unconditional"],
    [
      "run",
      "cargo build --locked --offline --bins\ncargo test --locked --offline --lib some_filter",
      "exact unconditional",
    ],
  ];
  for (const [field, value, expected] of cases) {
    const jobs = clone(realJobs("rust.yml"));
    const job = jobs["native-arm64"] as Mapping;
    if (["runs-on", "timeout-minutes", "continue-on-error", "strategy"].includes(field))
      job[field] = value;
    else ((job["steps"] as Mapping[]).at(-1) as Mapping)[field] = value;
    expect(joined(verifyNativeArm64Execution(jobs)), field).toContain(expected);
  }
  const jobs = clone(realJobs("rust.yml"));
  expect(((jobs["native-arm64"] as Mapping)["steps"] as Mapping[]).at(-1)?.["run"]).toBe(
    RUST_NATIVE_ARM64_RUN + "\n",
  );
  ((jobs["native-arm64"] as Mapping)["steps"] as Mapping[]).pop();
  expect(joined(verifyNativeArm64Execution(jobs))).toContain("exactly once");
});

test("old combined arm scheduler is rejected", () => {
  const errors = registryErrors([], (jobs) => {
    const native = jobs["native-arm64"] as Mapping;
    delete jobs["native-arm64"];
    const step = (native["steps"] as Mapping[]).at(-1) as Mapping;
    step["if"] = "runner.arch == 'ARM64' && matrix.shard == 'a'";
    ((jobs["postgres"] as Mapping)["steps"] as Mapping[]).splice(7, 0, step);
  });
  expect(joined(errors)).toContain("native-arm64 job missing");
});

test("selected install exact supported execution scope", () => {
  using tree = new RegistryTree(["selected_install_lifetime"]);
  const ctx = tree.context();
  expect(verifyRustSuiteRegistry(ctx)).toEqual([]);
  const jobs = (ctx.workflows["rust.yml"] as Mapping)["jobs"] as Mapping;
  expect(selectedInstallInventory(jobs)).toEqual([new Set(["selected_install_lifetime"]), null]);
  expect(namedStep(jobs["postgres"] as Mapping, RUST_SELECTED_INSTALL_STEP)["run"]).toContain(
    'prefix="fvoci-selected-install-", dir="/run"',
  );
});

test("selected install missing or masked execution fails", () => {
  const helperName = "Validate and restore finished postgres executables (no rebuild fallback)";
  const mutations: Record<string, (job: Mapping, step: Mapping) => void> = {
    missing: (job, step) =>
      void (job["steps"] = (job["steps"] as Mapping[]).filter((item) => item !== step)),
    duplicate: (job, step) => void (job["steps"] as Mapping[]).push({ ...step }),
    disabled: (_job, step) => void (step["if"] = "false"),
    masked: (_job, step) => void (step["continue-on-error"] = true),
    "no-run-only": (_job, step) =>
      void (step["run"] =
        "cargo test --features db-tests --test selected_install_lifetime --no-run"),
    filtered: (_job, step) =>
      void (step["run"] = (step["run"] as string).replace(
        '"--test-threads=1", "--nocapture"',
        '"nonexistent_filter", "--nocapture"',
      )),
    "zero-count": (_job, step) =>
      void (step["run"] = (step["run"] as string).replace("4 passed;", "0 passed;")),
    "wrong-helper-feature": (job) => {
      const helper = namedStep(job, helperName);
      helper["run"] = (helper["run"] as string).replace("--cohort postgres ", "--cohort helper ");
    },
    "helper-after-step": (job) => {
      const steps = job["steps"] as Mapping[];
      const helper = namedStep(job, helperName);
      steps.splice(steps.indexOf(helper), 1);
      steps.push(helper);
    },
    "missing-arm": (job) => {
      const rows = catalogRows(job);
      const row = rows.find(
        (item) => item["runner"] === "ubuntu-26.04-arm" && item["shard"] === "b",
      ) as Mapping;
      row["pg_major"] = "17";
      storeCatalog(job, rows);
    },
  };
  for (const [name, mutate] of Object.entries(mutations)) {
    const errors = registryErrors(["selected_install_lifetime"], (jobs) => {
      const job = jobs["postgres"] as Mapping;
      mutate(job, namedStep(job, RUST_SELECTED_INSTALL_STEP));
    });
    expect(errors.length, name).toBeGreaterThan(0);
    expect(joined(errors), name).toContain(
      name === "missing" || name === "duplicate" ? RUST_SELECTED_INSTALL_STEP : "selected install",
    );
  }
});

test("selected install cannot be double assigned as a PostgreSQL fixture", () => {
  const errors = registryErrors(["selected_install_lifetime"], (jobs) => {
    const job = jobs["postgres"] as Mapping;
    const rows = catalogRows(job);
    for (const row of rows)
      if (row["shard"] === "b")
        row["tests"] = `${row["tests"] as string} --test selected_install_lifetime`;
    storeCatalog(job, rows);
  });
  expect(joined(errors)).toContain("assigned to multiple CI buckets: selected_install_lifetime");
});

test("new cargo target without a CI row fails", () => {
  const text = joined(registryErrors(["missing_db_target_probe"]));
  expect(text).toContain("missing_db_target_probe");
  expect(text).toContain("missing from rust.yml inventory");
});

test("postgres arm64 row omission fails", () => {
  const errors = registryErrors([], (jobs) => {
    const job = jobs["postgres"] as Mapping;
    const rows = catalogRows(job);
    for (const row of rows) {
      if (row["runner"] === "ubuntu-26.04-arm")
        row["tests"] = (row["tests"] as string).replace(" --test search_meili", "");
    }
    storeCatalog(job, rows);
  });
  expect(errors).toContain("rust: postgres matrix missing on arm64: search_meili");
});

test("collaboration script inventory parses the maintained script", () => {
  const [tests, err] = collaborationScriptInventory(ROOT);
  expect(err).toBeNull();
  expect(tests?.size).toBe(10);
  expect(tests).toContain("task_collab_integration");
  expect(tests).toContain("collab_product");
});

const prefix = "cargo test --locked --offline --no-fail-fast --features db-tests \\\n";

test("collaboration script target in two invocations fails", () => {
  const body =
    prefix +
    "  --test collab_product \\\n  --test task_collab_integration \\\n  | tee log\n" +
    prefix +
    "  --test task_collab_integration \\\n  -- --test-threads=1 \\\n  | tee -a log\n";
  expect(collaborationScriptInventoryFromText("#!/usr/bin/env bash\n" + body)).toEqual([
    null,
    "rust: collaboration CI script runs --test targets more than once: task_collab_integration",
  ]);
});

test("collaboration script libtest suffix allows only scheduling", () => {
  for (const [suffix, allowed] of [
    ["--test-threads=1", true],
    ["--nocapture", true],
    ["some_filter", false],
    ["--skip personal_transfer", false],
    ["--ignored", false],
    ["--test-threads=4", false],
  ] as const) {
    const body =
      prefix + "  --test task_collab_integration \\\n  -- " + suffix + " \\\n  | tee log\n";
    const [tests, err] = collaborationScriptInventoryFromText("#!/usr/bin/env bash\n" + body);
    if (allowed) expect([tests, err], suffix).toEqual([new Set(["task_collab_integration"]), null]);
    else expect(err, suffix).toContain("libtest filter");
  }
});

test("collaboration script shape refusals", () => {
  const cases: [string, string][] = [
    ["echo nothing\n", "rust: collaboration CI script missing cargo test invocation"],
    [
      "cargo test --locked --offline --features db-tests \\\n  --test a\n",
      "must invoke cargo test with --features db-tests",
    ],
    [
      "cargo test --locked --offline --no-fail-fast --features db-tests --exclude x \\\n  --test a\n",
      "must not use --exclude",
    ],
    [prefix + "  --exclude x\n", "must declare at least one --test target"],
    [
      "cargo test --locked --offline --no-fail-fast --features db-tests --release \\\n  --test a\n",
      "unknown cargo test flag '--release'",
    ],
    [
      "cargo test --locked --offline --no-fail-fast --features db-tests \n",
      "must declare at least one --test target",
    ],
    [prefix + "  --test a \\\n  --test a\n", ""],
  ];
  for (const [body, needle] of cases) {
    const [, err] = collaborationScriptInventoryFromText(body);
    if (needle) expect(err, body).toContain(needle);
    else expect(err, body).toBeNull();
  }
});

test("missing collaboration script fails", () => {
  using tree = new RegistryTree([]);
  rmSync(join(tree.root, RUST_COLLAB_CI_SCRIPT));
  expect(verifyRustSuiteRegistry(tree.context())).toEqual([
    "rust: missing collaboration CI script scripts/run-rust-collaboration-ci-tests.sh",
  ]);
});

test("missing Cargo.toml and workflow fail instead of passing silently", () => {
  using tree = new RegistryTree([]);
  const ctx = tree.context();
  rmSync(join(tree.root, "Cargo.toml"));
  expect(verifyRustSuiteRegistry(ctx)).toEqual(["rust: missing root Cargo.toml"]);
  delete ctx.workflows["rust.yml"];
  expect(verifyRustSuiteRegistry(ctx)).toEqual(["rust: missing workflow file rust.yml"]);
});

test("autodiscovered root tests are classified by crate attributes", () => {
  for (const [body, needle] of [
    ['#![cfg(feature = "db-tests")]\n', "missing from rust.yml inventory: missing_db_target_probe"],
    [
      "//! pad\n".repeat(12) + '#![cfg(feature = "db-tests")]\n',
      "missing from rust.yml inventory: missing_db_target_probe",
    ],
    [
      '#[cfg(feature = "db-tests")]\nmod suite {}\n',
      "tests/missing_db_target_probe.rs is not registered and has no crate",
    ],
  ] as const) {
    using tree = new RegistryTree([]);
    tree.write("tests/missing_db_target_probe.rs", body);
    expect(joined(verifyRustSuiteRegistry(tree.context())), body).toContain(needle);
  }
  using tree = new RegistryTree([]);
  tree.write("tests/static_api.rs", "fn main() {}\n");
  tree.write("tests/native_only.rs", '#![cfg(feature = "extract-native-tests")]\n');
  tree.write("tests/empty.rs", "\n");
  expect(verifyRustSuiteRegistry(tree.context())).toEqual([]);
});

test("postgres decoy step without matrix execution fails", () => {
  using tree = new RegistryTree([]);
  const rel = ".github/workflows/rust.yml";
  const decoy =
    "      - name: PostgreSQL integration tests decoy\n        run: echo --test db_integration --features db-tests\n";
  tree.write(
    rel,
    tree
      .read(rel)
      .replace(
        "      - name: PostgreSQL integration tests\n",
        decoy + "      - name: PostgreSQL integration tests\n",
      )
      .replace(
        '        run: python3 scripts/ci_selection.py rust-binaries run --directory "$RUNNER_TEMP/rust-binaries" ${{ matrix.tests }}\n',
        '        run: python3 scripts/ci_selection.py rust-binaries run --directory "$RUNNER_TEMP/rust-binaries"\n',
      ),
  );
  expect(joined(verifyRustSuiteRegistry(tree.context()))).toContain("matrix.tests");
});

function stepMutation(job: string, stepName: string, mutate: (step: Mapping) => void) {
  return (jobs: Mapping) => {
    mutate(namedStep(jobs[job] as Mapping, stepName));
  };
}

test("execution step weakening fails", () => {
  const pg = (mutate: (step: Mapping) => void) =>
    stepMutation("postgres", RUST_POSTGRES_INTEGRATION_STEP, mutate);
  const cases: [string, (jobs: Mapping) => void, string][] = [
    ["pg if", pg((s) => void (s["if"] = "false")), "must not have an if condition"],
    ["pg coe", pg((s) => void (s["continue-on-error"] = true)), "continue-on-error"],
    ["pg coe string", pg((s) => void (s["continue-on-error"] = "true")), "continue-on-error"],
    ["pg no-run", pg((s) => void (s["run"] = (s["run"] as string) + " --no-run")), "--no-run"],
    [
      "pg exclude",
      pg(
        (s) =>
          void (s["run"] = (s["run"] as string).replace(
            "${{ matrix.tests }}",
            "--exclude fvoci-server ${{ matrix.tests }}",
          )),
      ),
      "--exclude",
    ],
    [
      "pg skip",
      pg((s) => void (s["run"] = (s["run"] as string) + " -- --skip '*'")),
      "libtest filter",
    ],
    [
      "pg or true",
      pg((s) => void (s["run"] = (s["run"] as string) + " || true")),
      "shell operator '||'",
    ],
    ["pg run type", pg((s) => void (s["run"] = ["x"])), "must have a string run command"],
    [
      "s3 if",
      stepMutation("postgres", RUST_S3_INTEGRATION_STEP, (s) => void (s["if"] = "false")),
      `rust: S3 integration step if must be "matrix.shard == 'b'", got 'false'`,
    ],
    [
      "s3 features",
      stepMutation("postgres", RUST_S3_INTEGRATION_STEP, (s) => {
        s["run"] =
          "bash scripts/start-test-minio.sh cargo test --locked --offline --no-fail-fast --test attachment_s3_integration";
      }),
      "S3 integration step",
    ],
    [
      "collab if",
      stepMutation("collaboration", RUST_COLLAB_INTEGRATION_STEP, (s) => void (s["if"] = "false")),
      "collaboration integration step must not have an if condition",
    ],
    [
      "collab echo",
      stepMutation(
        "collaboration",
        RUST_COLLAB_INTEGRATION_STEP,
        (s) => void (s["run"] = "echo bash scripts/run-rust-collaboration-ci-tests.sh"),
      ),
      "collaboration integration step must execute bash scripts/run-rust-collaboration-ci-tests.sh",
    ],
    [
      "collab runner",
      (jobs) => {
        const matrix = ((jobs["collaboration"] as Mapping)["strategy"] as Mapping)[
          "matrix"
        ] as Mapping;
        matrix["include"] = (matrix["include"] as Mapping[]).filter(
          (row) => row["runner"] !== "ubuntu-26.04-arm",
        );
      },
      "rust: collaboration matrix missing runners: ubuntu-26.04-arm",
    ],
    [
      "matrix no-run",
      (jobs) => {
        const job = jobs["postgres"] as Mapping;
        const rows = catalogRows(job);
        (rows[0] as Mapping)["tests"] = "--no-run --test db_integration";
        storeCatalog(job, rows);
      },
      "rust: postgres matrix tests must not use --no-run",
    ],
    [
      "matrix runner",
      (jobs) => {
        const job = jobs["postgres"] as Mapping;
        const rows = catalogRows(job);
        (rows[0] as Mapping)["runner"] = null;
        storeCatalog(job, rows);
      },
      "rust: postgres matrix row has unknown runner None",
    ],
  ];
  for (const [name, mutate, needle] of cases) {
    expect(joined(registryErrors([], mutate)), name).toContain(needle);
  }
});

test("postgres and S3 execution inventories on the real workflow", () => {
  const jobs = realJobs("rust.yml");
  expect(verifyPostgresIntegrationExecution(jobs)).toEqual([]);
  expect(postgresS3Inventory(jobs)).toEqual([new Set(["attachment_s3_integration"]), null]);
  expect(validateMatrixTestsFragment("--test a --test")).toBe(
    "rust: postgres matrix tests must be --test NAME pairs only",
  );
  expect(validateMatrixTestsFragment("--test a; --test b")).toBe(
    "rust: postgres matrix tests must not contain shell operator ';'",
  );
});

// A comment line holding one invalid UTF-8 byte: Python's strict read refuses
// it, so it must never decode to U+FFFD and pass as a harmless comment.
const INVALID_UTF8_LINE = Buffer.from([0x0a, 0x23, 0x20, 0xff, 0x0a]);

test("invalid UTF-8 in read inputs is a refusal", () => {
  for (const [rel, expected] of [
    [RUST_COLLAB_CI_SCRIPT, "rust: scripts/run-rust-collaboration-ci-tests.sh is not valid UTF-8"],
    ["Cargo.toml", "rust: Cargo.toml is not valid UTF-8"],
  ] as const) {
    using tree = new RegistryTree([]);
    appendFileSync(join(tree.root, rel), INVALID_UTF8_LINE);
    expect(verifyRustSuiteRegistry(tree.context()), rel).toEqual([expected]);
  }
  for (const stem of ["missing_db_target_probe", "static_api"]) {
    using tree = new RegistryTree([]);
    tree.write(`tests/${stem}.rs`, '#![cfg(feature = "db-tests")]\n');
    appendFileSync(join(tree.root, `tests/${stem}.rs`), INVALID_UTF8_LINE);
    expect(verifyRustSuiteRegistry(tree.context()), stem).toEqual([
      `rust: tests/${stem}.rs is not valid UTF-8`,
    ]);
  }
  using tree = new RegistryTree([]);
  tree.write("tests/bom.rs", "\ufeff\n");
  expect(joined(verifyRustSuiteRegistry(tree.context()))).toContain(
    "tests/bom.rs is not registered",
  );
});

const PYTHON_ONLY_WS = ["\u0085", "\u001c", "\u001d", "\u001e", "\u001f"];

test("target extraction and validation share Python whitespace", () => {
  for (const ws of PYTHON_ONLY_WS) {
    const label = `U+${ws.charCodeAt(0).toString(16).padStart(4, "0")}`;
    expect(cargoTestFlagsInText(`--test a${ws}--test${ws}b`), label).toEqual(new Set(["a", "b"]));
    expect(validateMatrixTestsFragment(`--test a${ws}--test b`), label).toBeNull();
    expect(validateMatrixTestsFragment(`--test a${ws}--test`), label).toBe(
      "rust: postgres matrix tests must be --test NAME pairs only",
    );
    const errors = registryErrors([], (jobs) => {
      const job = jobs["postgres"] as Mapping;
      const rows = catalogRows(job);
      for (const row of rows) {
        if (row["shard"] === "a")
          row["tests"] = `${row["tests"] as string}${ws}--test collab_product`;
      }
      storeCatalog(job, rows);
    });
    expect(errors, label).toEqual([
      "rust: integration target assigned to multiple CI buckets: collab_product",
    ]);
  }
  // U+FEFF is JS whitespace but not Python whitespace.
  expect(cargoTestFlagsInText("x\ufeff--test a")).toEqual(new Set());
  expect(validateMatrixTestsFragment("--test\ufeffa")).toBe(
    "rust: postgres matrix tests must be --test NAME pairs only",
  );
});
