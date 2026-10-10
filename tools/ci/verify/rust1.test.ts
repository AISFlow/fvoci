import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHash } from "node:crypto";
import { type Mapping, RUST_WORKFLOW_FILE, field } from "./rust-common.ts";
import { dbIntegrationTargets, rootDbIntegrationRegistryTargets } from "./rust-cargo-targets.ts";
import { verifyRustBinaryHandoff, verifyRustBinaryHandoffCtx } from "./rust-handoff.ts";
import {
  RUST_SELECTED_LIBRARY_FILTERS,
  selectedLibraryResultError,
  verifySelectedLibraryExecution,
  verifySelectedLibraryExecutionCtx,
} from "./rust-library.ts";
import {
  POSTGRES_MATRIX_EXPR,
  RUST_POSTGRES_BUDGET,
  cargoTestFlagsInText,
  matrixTestsFragmentError,
  postgresMatrixInventory,
  postgresMatrixJson,
  postgresMatrixRows,
  verifyPostgresBudgetMatrix,
  verifyPostgresBudgetMatrixCtx,
} from "./rust-postgres.ts";
import { verifySqlitePrefixCache } from "./rust-sqlite.ts";
import { RUST1_MUTATIONS, loadRustWorkflow } from "./rust1-mutations.ts";

const root = resolve(import.meta.dir, "../../..");
type Jobs = Record<string, Mapping>;
const fresh = () => loadRustWorkflow(root) as { jobs: Jobs };
const job = (jobs: Jobs, name: string): Mapping => {
  const found = jobs[name];
  if (!found) throw new Error(`fixture job ${name} missing`);
  return found;
};
const allErrors = (jobs: Mapping) => [
  ...verifySelectedLibraryExecution(jobs),
  ...verifyPostgresBudgetMatrix(jobs),
  ...verifyRustBinaryHandoff(jobs),
  ...[postgresMatrixInventory(jobs).error ?? []].flat(),
];

describe("real rust.yml", () => {
  test("every rust1 check passes and both architectures schedule the same targets", () => {
    const workflow = fresh();
    const ctx = { root, workflows: { [RUST_WORKFLOW_FILE]: workflow } };
    expect(verifySelectedLibraryExecutionCtx(ctx)).toEqual([]);
    expect(verifyPostgresBudgetMatrixCtx(ctx)).toEqual([]);
    expect(verifyRustBinaryHandoffCtx(ctx)).toEqual([]);
    const { perArch, error } = postgresMatrixInventory(workflow.jobs);
    expect(error).toBeNull();
    expect(perArch.x64.size).toBe(44);
    expect([...perArch.x64].sort()).toEqual([...perArch.arm64].sort());
  });

  test("absent rust.yml is left to the workflow-file check", () => {
    const ctx = { root, workflows: {} };
    expect(verifySelectedLibraryExecutionCtx(ctx)).toEqual([]);
    expect(verifyPostgresBudgetMatrixCtx(ctx)).toEqual([]);
    expect(verifyRustBinaryHandoffCtx(ctx)).toEqual([]);
  });

  test("budget keeps the measured A/B/C split on every platform/major pair", () => {
    const originalA = new Set([
      "db_integration",
      "task_integration",
      "collab_integration",
      "invitation_integration",
      "search_index",
      "comment_integration",
      "api_token_integration",
      "mail_integration",
      "task_labels_integration",
      "workspace_lifecycle",
      "search_query",
      "notification_integration",
      "push_integration",
      "schedule_ics_integration",
      "search_meili",
      "secret_maintenance_integration",
      "outbox_reset_integration",
      "pool_release_integration",
    ]);
    const moved = new Set(["task_integration", "comment_integration"]);
    const postgres = job(fresh().jobs, "postgres");
    const rows = postgresMatrixRows(postgres).value ?? [];
    expect(rows).toHaveLength(12);
    for (const [runner, major] of [
      ["ubuntu-26.04", "16"],
      ["ubuntu-26.04", "17"],
      ["ubuntu-26.04", "18"],
      ["ubuntu-26.04-arm", "18"],
    ]) {
      const shard = (name: string) => {
        const row = rows.find(
          (r) => r.runner === runner && r.pg_major === major && r.shard === name,
        );
        return cargoTestFlagsInText(String(row?.tests));
      };
      const [a, b, c] = [shard("a"), shard("b"), shard("c")];
      expect([...a].sort()).toEqual([...originalA].filter((name) => !moved.has(name)).sort());
      expect([...c].sort()).toEqual([...moved].sort());
      expect(b.size).toBe(26);
      expect(new Set([...a, ...b, ...c]).size).toBe(44);
    }
    expect(field(postgres.strategy, "matrix")).toBe(POSTGRES_MATRIX_EXPR);
    expect(field(field(postgres.services, "postgres"), "image")).toBe(
      "${{ matrix.postgres_image }}",
    );
  });
});

describe("mutation corpus", () => {
  test("covers at least thirty mutations with unique names", () => {
    expect(RUST1_MUTATIONS.length).toBeGreaterThanOrEqual(30);
    expect(new Set(RUST1_MUTATIONS.map((m) => m.name)).size).toBe(RUST1_MUTATIONS.length);
  });
  for (const mutation of RUST1_MUTATIONS) {
    test(mutation.name, () => {
      const workflow = fresh();
      mutation.mutate(workflow);
      const errors = allErrors(workflow.jobs);
      expect(errors.some((error) => error.includes(mutation.needle))).toBe(true);
    });
  }
});

describe("postgres budget", () => {
  test("refuses old, global, x64 and larger timeout budgets", () => {
    const budgets: unknown[] = [
      undefined,
      15,
      20,
      25,
      30,
      "${{ matrix.shard == 'b' && 20 || 15 }}",
      "${{ matrix.shard == 'b' && 25 || 15 }}",
      "${{ matrix.runner == 'ubuntu-26.04-arm' && 25 || 20 }}",
      RUST_POSTGRES_BUDGET.replace("ubuntu-26.04-arm", "ubuntu-26.04"),
      RUST_POSTGRES_BUDGET.replace("&& 25", "&& 26"),
      RUST_POSTGRES_BUDGET.replace("|| 20", "|| 25"),
      RUST_POSTGRES_BUDGET.replace("|| 15", "|| 20"),
    ];
    for (const budget of budgets) {
      const jobs = fresh().jobs;
      const postgres = job(jobs, "postgres");
      if (budget === undefined) delete postgres["timeout-minutes"];
      else postgres["timeout-minutes"] = budget;
      expect(verifyPostgresBudgetMatrix(jobs)).toContain(
        "rust: PostgreSQL budget must retain A/C15m, x64 B20m and ARM64 B25m",
      );
    }
  });

  test("missing postgres job is reported by budget and inventory", () => {
    const jobs = fresh().jobs;
    delete jobs.postgres;
    expect(verifyPostgresBudgetMatrix(jobs)).toEqual(["rust: PostgreSQL budget job missing"]);
    expect(postgresMatrixInventory(jobs).error).toBe("rust: postgres job missing");
  });

  test("tests fragments accept only --test NAME pairs", () => {
    expect(matrixTestsFragmentError("--test a --test b")).toBeNull();
    expect(matrixTestsFragmentError("  ")).toBe(
      "rust: postgres matrix row missing tests command fragment",
    );
    expect(matrixTestsFragmentError("--no-run --test a")).toBe(
      "rust: postgres matrix tests must not use --no-run",
    );
    expect(matrixTestsFragmentError("--test a --exclude b")).toBe(
      "rust: postgres matrix tests must not use --exclude",
    );
    expect(matrixTestsFragmentError("--test a && true")).toBe(
      "rust: postgres matrix tests must not contain shell operator '&&'",
    );
    expect(matrixTestsFragmentError("--test a -- --nocapture")).toBe(
      "rust: postgres matrix tests must be --test NAME pairs only",
    );
    expect(matrixTestsFragmentError("--test a -- --skip b")).toBe(
      "rust: postgres matrix tests must not use libtest filter after --",
    );
    expect(matrixTestsFragmentError("--test a.b")).toBe(
      "rust: postgres matrix tests must be --test NAME pairs only",
    );
    expect(matrixTestsFragmentError("--test\x1ca")).toBeNull();
  });

  test("pull requests run PG 18 x64 only; other events run the full catalog", () => {
    const rows = postgresMatrixRows(job(fresh().jobs, "postgres")).value ?? [];
    const include = (event: string) =>
      (JSON.parse(postgresMatrixJson(event, rows)) as { include: Mapping[] }).include;
    expect(include("pull_request").map((row) => [row.runner, row.pg_major])).toEqual([
      ["ubuntu-26.04", "18"],
      ["ubuntu-26.04", "18"],
      ["ubuntu-26.04", "18"],
    ]);
    expect(include("merge_group")).toEqual(rows);
    expect(postgresMatrixJson("push", [{ runner: "é" }])).toBe(
      '{"include":[{"runner":"\\u00e9"}]}',
    );
  });
});

describe("selected library", () => {
  const success = (name: string) =>
    "running 1 test\n" +
    `test ${name} ... ok\n` +
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out; finished in 0.01s\n";

  test("all 54 filters pass with a single successful exact result", () => {
    expect(RUST_SELECTED_LIBRARY_FILTERS).toHaveLength(54);
    expect(
      createHash("sha256")
        .update(RUST_SELECTED_LIBRARY_FILTERS.slice(0, 44).join("\n"))
        .digest("hex"),
    ).toBe("b1d7fff1c44346a300454eff3b0721e1a2078842308a1847b9e9411e23741cb1");
    for (const name of RUST_SELECTED_LIBRARY_FILTERS) {
      expect(selectedLibraryResultError(name, 0, success(name))).toBeNull();
    }
  });

  test("zero, ignored, wrong, extra and failed results are not a pass", () => {
    const name = RUST_SELECTED_LIBRARY_FILTERS[0] ?? "";
    const good = success(name);
    const cases: [number, string][] = [
      [0, ""],
      [0, "running 1 test\n"],
      [0, good.split("test result:")[0] ?? ""],
      [0, good.replace(`test ${name} ... ok\n`, "")],
      [1, good],
      [0, good.replace("running 1 test", "running 0 tests").replace("1 passed", "0 passed")],
      [
        0,
        good
          .replace("... ok", "... ignored")
          .replace("1 passed", "0 passed")
          .replace("0 ignored", "1 ignored"),
      ],
      [0, good.replace(name, "different::test")],
      [0, good.replace("running 1 test", "running 2 tests").replace("1 passed", "2 passed")],
      [0, good + "test different::test ... ok\n"],
      [0, good + good],
      [0, good.replace("0 measured", "1 measured")],
      [0, good.replace("0 failed", "1 failed")],
      [0, good.replace("\n", "\r\n")],
    ];
    for (const expected of RUST_SELECTED_LIBRARY_FILTERS) {
      for (const [code, output] of cases) {
        expect(
          selectedLibraryResultError(expected, code, output.replaceAll(name, expected)),
        ).not.toBeNull();
      }
    }
    // Only the count line differs: the running-count rule alone must refuse it.
    expect(
      selectedLibraryResultError(name, 0, good.replace("running 1 test", "running 2 tests")),
    ).toBe("rust: selected library command must run exactly one test");
    expect(selectedLibraryResultError("missing::filter", 0, success("missing::filter"))).toBe(
      "rust: unregistered selected library filter",
    );
  });

  test("registry removal, reorder, duplicate and unknown filters fail the slice pins", () => {
    const jobs = fresh().jobs;
    const all = RUST_SELECTED_LIBRARY_FILTERS;
    const swap = (at: number) =>
      [...all.slice(0, at), all[at + 1], all[at], ...all.slice(at + 2)] as string[];
    const variants: string[][] = [
      swap(0),
      swap(18),
      swap(28),
      swap(33),
      swap(35),
      swap(44),
      all.slice(0, 18),
      all.slice(0, 28),
      all.slice(0, 33),
      all.slice(0, 35),
      all.slice(0, 44),
    ];
    all.forEach((_, index) => variants.push(all.filter((__, i) => i !== index)));
    for (let index = 18; index < all.length; index++) {
      variants.push([...all.slice(0, index), all[index - 1] ?? "", ...all.slice(index + 1)]);
      variants.push([...all.slice(0, index), "unknown::filter", ...all.slice(index + 1)]);
    }
    for (const filters of variants) {
      expect(verifySelectedLibraryExecution(jobs, filters)).toContain(
        "rust: selected library registry must retain all54 exact filters (original44 prefix and member10)",
      );
    }
  });
});

describe("sqlite prefix and shapes", () => {
  test("prefix check passes alone and reports malformed steps instead of skipping them", () => {
    const jobs = fresh().jobs;
    expect(verifySqlitePrefixCache(jobs)).toEqual([]);
    (job(jobs, "postgres").steps as Mapping[]).push({ name: "x", with: null });
    expect(verifySqlitePrefixCache(jobs)).toEqual([
      "rust: postgres job must be a mapping whose steps are mappings with string run and with.path and a scalar name",
    ]);
  });
});

describe("cargo db-tests targets", () => {
  const stub = (extra: string[] = []) =>
    ["[features]", "db-tests = []", ""]
      .concat(
        ["db_integration", ...extra].flatMap((name) => [
          "[[test]]",
          `name = "${name}"`,
          `path = "tests/${name}.rs"`,
          'required-features = ["db-tests"]',
          "",
        ]),
      )
      .join("\n");

  test("explicit and autodiscovered db-tests crates register, item-level cfg does not", () => {
    expect([...(dbIntegrationTargets(stub(["probe"]), null).targets ?? [])]).toEqual([
      "db_integration",
      "probe",
    ]);
    const late = "//! pad\n".repeat(12) + '#![cfg(feature = "db-tests")]\n';
    expect(dbIntegrationTargets(stub(), [{ stem: "late", text: late }]).targets?.has("late")).toBe(
      true,
    );
    expect(
      dbIntegrationTargets(stub(), [
        { stem: "item", text: '#[cfg(feature = "db-tests")]\nmod suite {}\n' },
      ]).error,
    ).toBe(
      'rust: tests/item.rs is not registered and has no crate #![cfg(feature = "db-tests")]; add CI inventory or an explicit fast/native exclusion',
    );
    const excluded = [
      { stem: "static_api", text: "fn main() {}\n" },
      { stem: "empty", text: "\n" },
    ];
    expect(dbIntegrationTargets(stub(), excluded).error).toBeNull();
    expect(
      dbIntegrationTargets(stub() + "[package]\nautotests = false\n", [{ stem: "x", text: "x" }])
        .error,
    ).toBeNull();
    expect(dbIntegrationTargets("[[test]]\nname = 3\n", null).error).toBe(
      "rust: Cargo.toml [[test]] missing name",
    );
    expect(
      dbIntegrationTargets('[[test]]\nname = "a"\nrequired-features = "db-tests"\n', null).error,
    ).toBe("rust: Cargo.toml [[test]] a required-features must be a string list");
    expect(dbIntegrationTargets("[[test]\n", null).error).toStartWith(
      "rust: Cargo.toml parse failed: ",
    );
  });

  test("reads the root tree and refuses a missing Cargo.toml", () => {
    const dir = mkdtempSync(join(tmpdir(), "rust1-"));
    try {
      expect(rootDbIntegrationRegistryTargets(dir).error).toBe("rust: missing root Cargo.toml");
      writeFileSync(join(dir, "Cargo.toml"), stub());
      mkdirSync(join(dir, "tests"));
      writeFileSync(join(dir, "tests", "bad.rs"), new Uint8Array([0x23, 0xff, 0x0a]));
      expect(rootDbIntegrationRegistryTargets(dir).error).toBe(
        "rust: tests/bad.rs must be a readable UTF-8 file",
      );
      rmSync(join(dir, "tests", "bad.rs"));
      writeFileSync(join(dir, "tests", "probe.rs"), '#![cfg(feature = "db-tests")]\n');
      expect([...(rootDbIntegrationRegistryTargets(dir).targets ?? [])].sort()).toEqual([
        "db_integration",
        "probe",
      ]);
      expect(rootDbIntegrationRegistryTargets(root).error).toBeNull();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
