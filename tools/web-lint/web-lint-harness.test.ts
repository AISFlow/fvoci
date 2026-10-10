import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { formatCommands, formatDefaults } from "./format-web.ts";
import { lintCommands, lintDefaults } from "./lint-web.ts";
import {
  capture,
  executeSelected,
  main,
  selectTestIds,
  testIds,
} from "./test-eslint.ts";
import { exitCodeOf, repositoryRoot, runChecked, type SpawnResult } from "./web-lint-process.ts";

const root = repositoryRoot();
const proofIds = [
  "VueToolchain.test_actual_nuxt_autoimport_registration_boundary",
  "VueToolchain.test_actual_web_and_editor_declaration_preparation_is_node_free",
  "VueToolchain.test_browser_does_not_receive_node_or_bun_globals",
  "VueToolchain.test_browser_worker_and_i18n_reject_runtime_node_bun",
  "VueToolchain.test_bun_development_types_and_runtime",
  "VueToolchain.test_directives_unused_any_and_typed_promises_fail",
  "VueToolchain.test_dynamic_slots_have_no_unused_false_positive",
  "VueToolchain.test_exact_development_export_buffer_contract",
  "VueToolchain.test_formatter_keeps_import_order_text_and_tailwind",
  "VueToolchain.test_generated_sfc_types_and_failed_refresh_have_no_waiver",
  "VueToolchain.test_indexed_access_preserves_missing_route_fallbacks",
  "VueToolchain.test_multiple_declaration_projects_fail_closed_together",
  "VueToolchain.test_node_test_runner_failure_propagation_and_no_waiver",
  "VueToolchain.test_qualified_runtime_globals_and_local_names",
  "VueToolchain.test_runtime_and_positive_sfc",
  "VueToolchain.test_strict_script_template_props_emits_and_slots_types",
  "VueToolchain.test_unprepared_sfc_import_has_no_fake_fallback",
  "VueToolchain.test_unused_disables_warnings_and_unmatched_paths_fail",
  "VueToolchain.test_y_text_declared_string_contract_without_rule_allowance",
];

test("lint and format commands keep the shell prelude, fixed flags, and default scope", () => {
  expect(lintCommands([])).toEqual([
    ["bun", "--bun", "scripts/verify-web-tools.mjs"],
    ["bun", "--bun", "scripts/prepare-vue-lint-types.mjs"],
    ["bun", "--bun", "node_modules/eslint/bin/eslint.js", "--max-warnings=0", ...lintDefaults],
  ]);
  expect(formatCommands([])).toEqual([
    ["bun", "--bun", "scripts/verify-web-tools.mjs"],
    [
      "bun",
      "--bun",
      "node_modules/prettier/bin/prettier.cjs",
      "--check",
      "--ignore-path",
      ".prettierignore",
      ...formatDefaults,
    ],
  ]);
  expect(lintCommands(["-f", "stylish"])[2]).toEqual([
    "bun",
    "--bun",
    "node_modules/eslint/bin/eslint.js",
    "--max-warnings=0",
    ...lintDefaults,
    "-f",
    "stylish",
  ]);
  expect(formatCommands(["--write"])[1]?.slice(0, 7)).toEqual([
    "bun",
    "--bun",
    "node_modules/prettier/bin/prettier.cjs",
    "--check",
    "--ignore-path",
    ".prettierignore",
    formatDefaults[0],
  ]);
  expect(lintCommands(["apps/web/src/vue/main.ts"])[2]?.slice(4)).toEqual(["apps/web/src/vue/main.ts"]);
  expect(formatCommands(["package.json"])[1]?.slice(6)).toEqual(["package.json"]);
  expect(lintCommands([""])[2]?.slice(4)).toEqual([""]);
  expect(formatCommands(["-"])[1]?.slice(6, 6 + formatDefaults.length)).toEqual(formatDefaults);
});

test("checked commands run from the repository root and stop on the first non-zero status", () => {
  const seen: { command: string[]; cwd: string }[] = [];
  const code = runChecked(lintCommands(["apps/web/src/vue/main.ts"]), root, (command, cwd) => {
    seen.push({ command: [...command], cwd });
    return { status: 4, signal: null };
  });
  expect(code).toBe(4);
  expect(seen).toEqual([{ command: ["bun", "--bun", "scripts/verify-web-tools.mjs"], cwd: root }]);
  let calls = 0;
  expect(runChecked([["one"], ["two"]], root, () => {
    calls += 1;
    return { status: 0, signal: null };
  })).toBe(0);
  expect(calls).toBe(2);
  expect(runChecked([["last"]], root, () => ({ status: 2, signal: null }))).toBe(2);
});

test("spawn failures use the shell status codes", () => {
  expect(exitCodeOf({ status: null, signal: "SIGTERM" })).toBe(143);
  expect(exitCodeOf({ status: null, signal: null, error: Object.assign(new Error("missing"), { code: "ENOENT" }) })).toBe(127);
  expect(exitCodeOf({ status: null, signal: null, error: Object.assign(new Error("denied"), { code: "EACCES" }) })).toBe(126);
  expect(exitCodeOf({ status: null, signal: null, error: Object.assign(new Error("other"), { code: "EIO" }) })).toBe(1);
  const missing: SpawnResult = spawnSync("fvoci-missing-bin-7fd3", [], { stdio: "ignore" }) as SpawnResult;
  // Real ENOENT is observed through the same helper the CLIs use.
  expect(exitCodeOf({
    status: missing.status,
    signal: missing.signal,
    error: missing.error,
  })).toBe(127);
});

test("fixture stdout above 64KiB is kept and the child status is returned", () => {
  const size = 70_000;
  const result = capture(["bun", "-e", `process.stdout.write("x".repeat(${size})); process.stderr.write("err");`]);
  expect(result.returncode).toBe(0);
  expect(result.stdout).toBe("x".repeat(size));
  expect(result.stderr).toBe("err");
  expect(capture(["bun", "-e", "process.exit(3)"]).returncode).toBe(3);
});

test("fixture selectors accept the documented unittest ids and reject flags", () => {
  expect(testIds).toEqual(proofIds);
  expect(selectTestIds([], proofIds)).toEqual({ ok: true, ids: proofIds });
  expect(selectTestIds(["VueToolchain.test_runtime_and_positive_sfc"], proofIds)).toEqual({
    ok: true,
    ids: ["VueToolchain.test_runtime_and_positive_sfc"],
  });
  expect(selectTestIds(["__main__.VueToolchain.test_formatter_keeps_import_order_text_and_tailwind"], proofIds).ok).toBe(true);
  expect(selectTestIds(["test_y_text_declared_string_contract_without_rule_allowance"], proofIds)).toEqual({
    ok: true,
    ids: ["VueToolchain.test_y_text_declared_string_contract_without_rule_allowance"],
  });
  expect(selectTestIds(["VueToolchain"], proofIds)).toEqual({ ok: true, ids: proofIds });
  expect(selectTestIds(["VueToolchain.test_runtime_and_positive_sfc", "VueToolchain"], proofIds).ok &&
    (selectTestIds(["VueToolchain.test_runtime_and_positive_sfc", "VueToolchain"], proofIds) as { ids: string[] }).ids[0],
  ).toBe("VueToolchain.test_runtime_and_positive_sfc");
  expect(selectTestIds(["VueToolchain.missing"], proofIds)).toEqual({ ok: false, status: 1, message: "no such test: VueToolchain.missing" });
  expect(selectTestIds(["-v"], proofIds).status).toBe(2);
  expect(main(["VueToolchain.missing"])).toBe(1);
  expect(main(["-k", "runtime"])).toBe(2);
});

test("fixture runner reports unittest success and failure statuses", () => {
  const cases = new Map<string, () => void>([
    ["VueToolchain.test_runtime_and_positive_sfc", () => {}],
    ["VueToolchain.test_dynamic_slots_have_no_unused_false_positive", () => {
      throw new Error("slot broke");
    }],
  ]);
  let passed = "";
  expect(executeSelected(["VueToolchain.test_runtime_and_positive_sfc"], cases, (chunk) => {
    passed += chunk;
  })).toBe(0);
  expect(passed).toContain("test_runtime_and_positive_sfc (__main__.VueToolchain.test_runtime_and_positive_sfc) ... ok\n");
  expect(passed).toMatch(/Ran 1 test in \d+\.\d{3}s\n\nOK\n$/);
  let failed = "";
  let cleaned = false;
  expect(executeSelected(["VueToolchain.test_dynamic_slots_have_no_unused_false_positive"], cases, (chunk) => {
    failed += chunk;
  }, { setup() {}, cleanup() { cleaned = true; } })).toBe(1);
  expect(cleaned).toBe(true);
  expect(failed).toContain("FAIL: test_dynamic_slots_have_no_unused_false_positive (__main__.VueToolchain.test_dynamic_slots_have_no_unused_false_positive)");
  expect(failed).toContain("FAILED (failures=1)\n");
});

test("format-web exit code follows prettier from another working directory", () => {
  const directory = mkdtempSync(join(tmpdir(), "fvoci-format-web-"));
  try {
    const good = join(directory, "good.ts");
    const bad = join(directory, "bad.ts");
    writeFileSync(good, "export const value = 1;\n");
    writeFileSync(bad, "export const value=1\n");
    const ok = spawnSync("bun", [join(root, "tools/web-lint/format-web.ts"), good], { cwd: "/tmp", encoding: "utf8" });
    const broken = spawnSync("bun", [join(root, "tools/web-lint/format-web.ts"), bad], { cwd: "/tmp", encoding: "utf8" });
    expect(ok.status).toBe(0);
    expect(broken.status).not.toBe(0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
