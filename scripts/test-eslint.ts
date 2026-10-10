// Positive and negative proofs for the pinned Bun-only Vue toolchain.
import { existsSync, mkdtempSync, openSync, closeSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { repositoryRoot } from "./web-lint-process.ts";

const separator1 = "======================================================================";
const separator2 = "----------------------------------------------------------------------";
const root = repositoryRoot();
const eslint = ["bun", "--bun", join(root, "node_modules/eslint/bin/eslint.js")];
const prettier = ["bun", "--bun", join(root, "node_modules/prettier/bin/prettier.cjs")];

export type Completed = { returncode: number; stdout: string; stderr: string };

type LintMessage = { ruleId: string | null; message: string; severity: number };
type LintFile = {
  messages: LintMessage[];
  errorCount: number;
  warningCount: number;
  fatalErrorCount: number;
};
type PrintConfig = {
  languageOptions?: { globals?: Record<string, string> };
  rules: Record<string, unknown>;
};

export function capture(
  args: readonly string[],
  options: { input?: string; timeoutMs?: number } = {},
): Completed {
  const command = args[0];
  if (!command) throw new Error("capture requires a command");
  const directory = mkdtempSync(join(tmpdir(), "eslint-proof-out-"));
  const stdoutPath = join(directory, "stdout.txt");
  const fd = openSync(stdoutPath, "w");
  let closed = false;
  try {
    const result = spawnSync(command, args.slice(1), {
      cwd: root,
      timeout: options.timeoutMs,
      encoding: "utf8",
      stdio: [options.input === undefined ? "inherit" : "pipe", fd, "pipe"],
      ...(options.input === undefined ? {} : { input: options.input }),
    });
    closeSync(fd);
    closed = true;
    if (result.error) {
      const stderr = typeof result.stderr === "string" ? result.stderr : "";
      throw new Error(`${result.error.message}\n${stderr}`);
    }
    return {
      returncode: result.status ?? 1,
      stdout: readFileSync(stdoutPath, "utf8"),
      stderr: typeof result.stderr === "string" ? result.stderr : "",
    };
  } finally {
    if (!closed) {
      try {
        closeSync(fd);
      } catch {
        // The fd is closed after a successful wait.
      }
    }
    rmSync(directory, { recursive: true, force: true });
  }
}

function fail(message: string): never {
  const error = new Error(message);
  error.name = "AssertionError";
  throw error;
}

function detailText(detail: unknown): string {
  if (detail === undefined) return "";
  return `\n${typeof detail === "string" ? detail : JSON.stringify(detail)}`;
}

function assertEqual(actual: unknown, expected: unknown, detail?: unknown): void {
  const same =
    typeof actual === "object" && actual !== null
      ? JSON.stringify(actual) === JSON.stringify(expected)
      : Object.is(actual, expected);
  if (!same) fail(`${JSON.stringify(actual)} != ${JSON.stringify(expected)}${detailText(detail)}`);
}

function assertNotEqual(actual: unknown, expected: unknown, detail?: unknown): void {
  const same =
    typeof actual === "object" && actual !== null
      ? JSON.stringify(actual) === JSON.stringify(expected)
      : Object.is(actual, expected);
  if (same) fail(`${JSON.stringify(actual)} == ${JSON.stringify(expected)}${detailText(detail)}`);
}

function assertIn(needle: string, haystack: string | Iterable<string>, detail?: unknown): void {
  const ok = typeof haystack === "string" ? haystack.includes(needle) : new Set(haystack).has(needle);
  if (!ok) fail(`${JSON.stringify(needle)} not found${detailText(detail)}`);
}

function assertNotIn(needle: string, haystack: Iterable<string> | Record<string, unknown>, detail?: unknown): void {
  const ok = Symbol.iterator in haystack
    ? new Set(haystack as Iterable<string>).has(needle)
    : Object.prototype.hasOwnProperty.call(haystack, needle);
  if (ok) fail(`${JSON.stringify(needle)} unexpectedly present${detailText(detail)}`);
}

function assertTrue(value: boolean, detail?: unknown): void {
  if (!value) fail(`expected true${detailText(detail)}`);
}

function assertGreater(actual: number, expected: number, detail?: unknown): void {
  if (!(actual > expected)) fail(`${actual} <= ${expected}${detailText(detail)}`);
}

function assertLess(actual: number, expected: number, detail?: unknown): void {
  if (!(actual < expected)) fail(`${actual} >= ${expected}${detailText(detail)}`);
}

function assertSetEqual(actual: Iterable<string>, expected: Iterable<string>, detail?: unknown): void {
  assertEqual([...new Set(actual)].sort(), [...new Set(expected)].sort(), detail);
}

function each<T>(items: readonly T[], fn: (item: T) => void): void {
  const failures: string[] = [];
  for (const item of items) {
    try {
      fn(item);
    } catch (error) {
      const label = Array.isArray(item) ? String(item[0]) : "";
      const message = error instanceof Error ? error.message : String(error);
      failures.push(label ? `${label}: ${message}` : message);
    }
  }
  if (failures.length) fail(failures.join("\n"));
}

function ruleOptions(config: PrintConfig, rule: string): { globals?: string[]; checkGlobalObject?: boolean; ignoreVoid?: boolean } {
  const value = config.rules[rule];
  if (!Array.isArray(value) || value.length < 2 || typeof value[1] !== "object" || value[1] === null) {
    fail(`missing options for ${rule}`);
  }
  return value[1] as { globals?: string[]; checkGlobalObject?: boolean; ignoreVoid?: boolean };
}

function createApi(directory: string) {
  function source(name: string, contents: string): string {
    const path = join(directory, name);
    writeFileSync(path, contents);
    return path;
  }
  function lint(name: string, contents: string, ...options: string[]) {
    const path = source(name, contents);
    const result = capture([...eslint, path, "--max-warnings=0", "--format=json", ...options]);
    return { result, report: JSON.parse(result.stdout) as LintFile[] };
  }
  function assertRule(name: string, contents: string, rule: string): void {
    const { result, report } = lint(name, contents);
    assertNotEqual(result.returncode, 0, report);
    assertIn(rule, new Set(report.flatMap((file) => file.messages.map((message) => message.ruleId ?? ""))), report);
  }
  function compiler(config: string, contents: string, extension = "vue", tool = "vue-tsc") {
    const path = source(`TypeProof.${extension}`, contents);
    const configPath = join(directory, "tsconfig.json");
    writeFileSync(configPath, JSON.stringify({
      extends: join(root, config),
      compilerOptions: { incremental: false, composite: false },
      include: [path],
      exclude: [],
    }));
    return capture(["bun", "--bun", join(root, "node_modules/.bin", tool), "--noEmit", "-p", configPath]);
  }
  return {
    directory,
    source,
    lint,
    assertRule,
    compiler,
    cleanup() {
      rmSync(directory, { recursive: true, force: true });
    },
  };
}

type Api = ReturnType<typeof createApi>;
let api: Api | undefined;
function useApi(): Api {
  if (!api) throw new Error("fixture setup did not run");
  return api;
}

const runtimePaths = [
  ["apps/web/src/vue/features/settings/toggle.ts", "browser"],
  ["apps/web/src/features/attachments/hwp-worker.ts", "worker"],
  ["apps/web/src/features/attachments/pptx-worker.ts", "worker"],
  ["apps/web/src/features/attachments/xlsx-worker.ts", "worker"],
  ["apps/web/public/sw.js", "worker"],
  ["packages/i18n/src/index.ts", "library"],
] as const;

const forbiddenGlobals = [
  "Bun", "process", "Buffer", "require", "__dirname", "__filename", "module", "exports", "setImmediate", "clearImmediate",
];

function testRuntimeAndPositiveSfc(): void {
  const { lint, compiler, source } = useApi();
  const runtime = capture(["bun", "-e", "console.log(JSON.stringify({bun:process.versions.bun,execPath:process.execPath}))"]);
  assertEqual(JSON.parse(runtime.stdout).bun, "1.4.2");
  const { result, report } = lint("ProofValid.vue", valid);
  assertEqual(result.returncode, 0, report);
  for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]) {
    const typed = compiler(config, valid);
    assertEqual(typed.returncode, 0, typed.stdout + typed.stderr);
  }
  const formatted = capture([...prettier, "--stdin-filepath", join(useApi().directory, "ProofValid.vue")], { input: valid });
  assertEqual(formatted.returncode, 0, formatted.stderr);
  assertIn("color: red;\n", formatted.stdout);
  assertIn(':key="row.id"', formatted.stdout);
  assertIn('emit("select", id);\n', formatted.stdout);
  const path = source("ProofValid.vue", formatted.stdout);
  const checked = capture([...prettier, "--check", path]);
  assertEqual(checked.returncode, 0, checked.stdout + checked.stderr);
  writeFileSync(path, valid);
  assertNotEqual(capture([...prettier, "--check", path]).returncode, 0);
}

function testDynamicSlots(): void {
  const source = `<script setup lang="ts">
import ProofChild from "./ProofChild.vue";
const slotName = "default";
</script>
<template><ProofChild label="ok"><template #[slotName]="{ row }"><span>{{ row.label }}</span></template></ProofChild></template>`;
  const { result, report } = useApi().lint("ProofDynamic.vue", source);
  assertEqual(result.returncode, 0, report);
  const typed = useApi().compiler("apps/web/tsconfig.vue.json", source.replace("#[slotName]", "#[missingSlotName]"));
  assertNotEqual(typed.returncode, 0);
  assertIn("missingSlotName", typed.stdout);
}

function testDirectivesUnusedAnyAndTypedPromisesFail(): void {
  const cases = [
    ["ProofUnused.vue", '<script setup lang="ts">const unused = 1;</script><template><p>ok</p></template>', "@typescript-eslint/no-unused-vars"],
    ["ProofUnusedImport.vue", '<script setup lang="ts">import { ref } from "vue";</script><template><p>ok</p></template>', "@typescript-eslint/no-unused-vars"],
    ["ProofParse.vue", '<template><p v-if="(">bad</p></template>', "vue/no-parsing-error"],
    ["ProofFor.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows">{{ row }}</p></template>', "vue/require-v-for-key"],
    ["ProofKey.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows" :key="1">{{ row }}</p></template>', "vue/valid-v-for"],
    ["ProofIfFor.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-if="true" v-for="row in rows" :key="row">{{ row }}</p></template>', "vue/no-use-v-if-with-v-for"],
    ["ProofIf.vue", "<template><p v-if>bad</p></template>", "vue/valid-v-if"],
    ["ProofModel.vue", '<template><input v-model="1" /></template>', "vue/valid-v-model"],
    ["ProofAny.ts", "export const value: any = 1;", "@typescript-eslint/no-explicit-any"],
    ["ProofUnsafe.ts", "export function read(value: any): string { return value; }", "@typescript-eslint/no-unsafe-return"],
    ["ProofPromise.ts", "export const save = (): Promise<number> => Promise.resolve(1); save();", "@typescript-eslint/no-floating-promises"],
    ["ProofVoid.ts", "export const save = (): Promise<number> => Promise.resolve(1); void save();", "@typescript-eslint/no-floating-promises"],
  ] as const;
  each(cases, ([name, contents, rule]) => useApi().assertRule(name, contents, rule));
}

function testStrictScriptTemplatePropsEmitsAndSlotsTypes(): void {
  const cases = [
    ['<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>', "TS2322"],
    ['<script setup lang="ts">const value = 1;</script><template><p>{{ value.toUpperCase() }}</p></template>', "TS2339"],
    ["<template><p>{{ missingTemplateName }}</p></template>", "TS2339"],
    ["<template><UnknownComponent /></template>", "UnknownComponent"],
    ['<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild :label="1" /></template>', "TS2322"],
    ['<script setup lang="ts">const emit = defineEmits<{ select: [id: number] }>(); emit("select", "bad");</script><template><p>bad</p></template>', "TS2345"],
    ['<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild label="ok" v-slot="{ row }"><p>{{ row.missing }}</p></ProofChild></template>', "TS2339"],
  ] as const;
  const failures: string[] = [];
  for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]) {
    for (const [contents, expected] of cases) {
      try {
        const result = useApi().compiler(config, contents);
        assertNotEqual(result.returncode, 0, result.stdout + result.stderr);
        assertIn(expected, result.stdout + result.stderr);
      } catch (error) {
        failures.push(`${config} ${expected}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }
  for (const config of ["apps/web/tsconfig.app.json", "packages/editor/tsconfig.json"]) {
    try {
      const result = useApi().compiler(config, 'export const value: number = "bad";', "ts", "tsc");
      assertNotEqual(result.returncode, 0);
      assertIn("TS2322", result.stdout);
    } catch (error) {
      failures.push(`${config}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  if (failures.length) fail(failures.join("\n"));
}

function testActualNuxtAutoimportRegistrationBoundary(): void {
  const result = useApi().compiler("apps/web/tsconfig.vue.json", '<template><UButton type="button">ok</UButton></template>');
  assertNotEqual(result.returncode, 0);
  assertIn("UButton", result.stdout);
}

function testUnusedDisablesWarningsAndUnmatchedPathsFail(): void {
  const disabled = useApi().lint("ProofDisable.ts", "// eslint-disable-next-line no-debugger\nexport const value = 1;\n");
  assertNotEqual(disabled.result.returncode, 0);
  assertTrue(disabled.report.some((file) => file.messages.some((message) => message.message.includes("Unused eslint-disable") && message.severity === 2)));
  const warning = useApi().lint("ProofWarning.vue", `<template><div v-html="'content'" /></template>`);
  assertEqual(warning.report.reduce((sum, file) => sum + file.errorCount, 0), 0, warning.report);
  assertGreater(warning.report.reduce((sum, file) => sum + file.warningCount, 0), 0, warning.report);
  assertNotEqual(warning.result.returncode, 0);
  assertNotEqual(capture([...eslint, join(useApi().directory, "missing.ts")]).returncode, 0);
  assertNotEqual(capture([...prettier, "--check", join(useApi().directory, "missing.ts")]).returncode, 0);
}

function testBrowserDoesNotReceiveNodeOrBunGlobals(): void {
  each(runtimePaths, ([path, environment]) => {
    const result = capture([...eslint, "--print-config", path]);
    assertEqual(result.returncode, 0, result.stderr);
    const config = JSON.parse(result.stdout) as PrintConfig;
    const declared = config.languageOptions?.globals ?? {};
    const overlap = forbiddenGlobals.filter((name) => Object.prototype.hasOwnProperty.call(declared, name));
    if (overlap.length) fail(`forbidden globals ${overlap.join(",")}${detailText(declared)}`);
    const runtimeRule = ruleOptions(config, "no-restricted-globals");
    const expected = new Set(environment === "worker" ? [...forbiddenGlobals, "window"] : forbiddenGlobals);
    assertSetEqual(runtimeRule.globals ?? [], expected);
    assertEqual(runtimeRule.checkGlobalObject, true);
    if (environment === "browser") assertIn("window", Object.keys(declared));
    else if (environment === "worker") {
      assertIn("self", Object.keys(declared));
      assertIn("postMessage", Object.keys(declared));
      assertNotIn("window", declared);
    }
    if (path.endsWith(".ts")) assertEqual(ruleOptions(config, "@typescript-eslint/no-floating-promises").ignoreVoid, false);
  });
}

function testBrowserWorkerAndI18nRejectRuntimeNodeBun(): void {
  const negative = `export const forbidden = [process.pid, Bun.version,
Buffer.alloc(0), require("bad"), __dirname, __filename, module, exports,
setImmediate, clearImmediate];`;
  const names = new Set(forbiddenGlobals);
  each(runtimePaths, ([path, environment]) => {
    const positive = {
      browser: "export const href = window.location.href;",
      worker: 'self.postMessage("ready");',
      library: "export const add = (left: number, right: number): number => left + right;",
    }[environment];
    const args = [...eslint, "--stdin", "--stdin-filename", path, "--max-warnings=0", "--format=json"];
    const validResult = capture(args, { input: positive });
    assertEqual(validResult.returncode, 0, validResult.stdout + validResult.stderr);
    const invalid = capture(args, { input: negative });
    const report = JSON.parse(invalid.stdout) as LintFile[];
    assertNotEqual(invalid.returncode, 0, report);
    assertEqual(report.reduce((sum, file) => sum + file.fatalErrorCount, 0), 0, report);
    const restricted = report.flatMap((file) => file.messages).filter((message) => message.ruleId === "no-restricted-globals");
    assertSetEqual(restricted.map((message) => message.message.split("'")[1] ?? ""), names, report);
    assertTrue(restricted.every((message) => message.severity === 2));
  });
}

function testQualifiedRuntimeGlobalsAndLocalNames(): void {
  each(["apps/web/src/vue/features/settings/toggle.ts", "apps/web/src/features/attachments/hwp-worker.ts", "packages/i18n/src/index.ts"], (path) => {
    const args = [...eslint, "--stdin", "--stdin-filename", path, "--max-warnings=0", "--format=json"];
    const positive = `export function local(process: { pid: number }): number { return process.pid; }
export function localObject(window: { process: { pid: number } }): number { return window.process.pid; }
export const label = { process: "local", Bun: "local" };`;
    const validResult = capture(args, { input: positive });
    assertEqual(validResult.returncode, 0, validResult.stdout + validResult.stderr);
    const negative = `export const qualified = [globalThis.process.pid,
window.process.pid, self.process.pid, globalThis.Bun.version,
window.Bun.version, self.Bun.version];`;
    const invalid = capture(args, { input: negative });
    const report = JSON.parse(invalid.stdout) as LintFile[];
    assertNotEqual(invalid.returncode, 0, report);
    assertEqual(report.reduce((sum, file) => sum + file.fatalErrorCount, 0), 0, report);
    const restricted = report.flatMap((file) => file.messages).filter((message) => message.ruleId === "no-restricted-globals");
    assertEqual(restricted.length, 6, report);
    assertTrue(restricted.every((message) => message.severity === 2));
  });
}

function testIndexedAccessPreservesMissingRouteFallbacks(): void {
  const source = `<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";
const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
</script><template><p>{{ slug }}</p></template>`;
  const linted = useApi().lint("ProofRoute.vue", source);
  assertEqual(linted.result.returncode, 0, linted.report);
  const dictionary = 'export function read(values: Record<string, string>): string { return values.slug ?? ""; }';
  const typedLint = useApi().lint("ProofDictionary.ts", dictionary);
  assertEqual(typedLint.result.returncode, 0, typedLint.report);
  const validResult = useApi().compiler("apps/web/tsconfig.eslint.json", dictionary, "ts", "tsc");
  assertEqual(validResult.returncode, 0, validResult.stdout + validResult.stderr);
  const invalid = useApi().compiler("apps/web/tsconfig.eslint.json", dictionary.replace('values.slug ?? ""', "values.slug"), "ts", "tsc");
  assertNotEqual(invalid.returncode, 0);
  assertIn("TS2322", invalid.stdout + invalid.stderr);
  assertIn("undefined", invalid.stdout + invalid.stderr);
  useApi().assertRule("ProofKnownField.ts", 'export function read(value: { slug: string }): string { return value.slug ?? ""; }', "@typescript-eslint/no-unnecessary-condition");
}

function testExactDevelopmentExportBufferContract(): void {
  each(["packages/editor/src/export/docx.ts", "packages/editor/src/export/pptx.ts"], (path) => {
    const args = [...eslint, "--stdin", "--stdin-filename", path, "--max-warnings=0", "--format=json"];
    const validResult = capture(args, { input: 'export const bytes = Buffer.from("oracle", "utf8");' });
    assertEqual(validResult.returncode, 0, validResult.stdout + validResult.stderr);
    const config = JSON.parse(capture([...eslint, "--print-config", path]).stdout) as PrintConfig;
    assertEqual(config.languageOptions?.globals?.Buffer, "readonly");
    assertSetEqual(ruleOptions(config, "no-restricted-globals").globals ?? [], [
      "Bun", "process", "require", "__dirname", "__filename", "module", "exports", "setImmediate", "clearImmediate",
    ]);
    const invalid = capture(args, { input: "export const forbidden = [process.pid, Bun.version, globalThis.process.pid, globalThis.Bun.version];" });
    const report = JSON.parse(invalid.stdout) as LintFile[];
    assertNotEqual(invalid.returncode, 0, report);
    assertEqual(report.reduce((sum, file) => sum + file.fatalErrorCount, 0), 0, report);
    const restricted = report.flatMap((file) => file.messages).filter((message) => message.ruleId === "no-restricted-globals");
    assertEqual(restricted.length, 4, report);
  });
  each(["packages/editor/src/json.ts", "packages/editor/src/export/limits.ts", "apps/web/src/features/attachments/hwp-worker.ts", "packages/i18n/src/index.ts"], (path) => {
    const result = capture([...eslint, "--stdin", "--stdin-filename", path, "--max-warnings=0", "--format=json"], { input: "export const bytes = Buffer.alloc(0);" });
    const report = JSON.parse(result.stdout) as LintFile[];
    assertNotEqual(result.returncode, 0, report);
    assertIn("no-restricted-globals", new Set(report.flatMap((file) => file.messages.map((message) => message.ruleId ?? ""))), report);
  });
}

function testBunDevelopmentTypesAndRuntime(): void {
  const source = `import assert from "node:assert/strict";
import { mock, test } from "bun:test";
await mock.module("fvoci-bun-typing-proof", () => ({ value: 1 }));
test("typed Bun Transpiler and module mock", () => {
  const code = new Bun.Transpiler({ loader: "ts" }).transformSync("export const value: number = 1;");
  assert.equal(typeof code, "string");
  assert.equal(code.includes("number"), false);
});`;
  const linted = useApi().lint("ProofBun.test.ts", source);
  assertEqual(linted.result.returncode, 0, linted.report);
  const path = join(useApi().directory, "ProofBun.test.ts");
  const runtime = capture(["bun", "test", "--isolate", path], { timeoutMs: 30_000 });
  assertEqual(runtime.returncode, 0, runtime.stdout + runtime.stderr);
  assertIn("1 pass", runtime.stderr);
  for (const config of ["apps/web/tsconfig.eslint.json", "packages/editor/tsconfig.eslint.json"]) {
    const validResult = useApi().compiler(config, source, "ts", "tsc");
    assertEqual(validResult.returncode, 0, validResult.stdout + validResult.stderr);
    const invalid = useApi().compiler(config, source.replace('loader: "ts"', 'loader: "invalid-loader"'), "ts", "tsc");
    assertNotEqual(invalid.returncode, 0);
    assertIn("TS2322", invalid.stdout + invalid.stderr);
  }
  useApi().assertRule("ProofBunUnsafe.test.ts", 'import { mock } from "bun:test"; mock.module("proof", () => ({ value: 1 }));', "@typescript-eslint/no-floating-promises");
  useApi().assertRule("ProofBunUnrelated.test.ts", 'import { test } from "bun:test"; test("proof", () => { Promise.resolve(1); });', "@typescript-eslint/no-floating-promises");
}

function testNodeTestRunnerFailurePropagationAndNoWaiver(): void {
  each([
    ["Pass", '() => { if (Number("1") !== 1) throw new Error("unexpected"); }', 0],
    ["Throw", '() => { throw new Error("node-test-throw-proof"); }', 1],
    ["Reject", '() => Promise.reject(new Error("node-test-reject-proof"))', 1],
  ] as const, ([name, body, expected]) => {
    const contents = `import test from "node:test"; test("${name}", ${body});`;
    const path = useApi().source(`NodeRunner${name}.test.ts`, contents);
    const runtime = capture(["bun", "test", "--isolate", path], { timeoutMs: 30_000 });
    assertEqual(runtime.returncode, expected, runtime.stdout + runtime.stderr);
    assertIn(expected === 0 ? "1 pass" : "1 fail", runtime.stderr);
  });
  useApi().assertRule("NodeUnhandled.test.ts", 'import test from "node:test"; await test("parent", (t) => { t.test("child", () => Promise.resolve()); });', "@typescript-eslint/no-floating-promises");
  const unrelated = 'import test from "node:test"; await test("parent", () => { Promise.reject(new Error("unhandled-promise-proof")); });';
  useApi().assertRule("NodeUnrelated.test.ts", unrelated, "@typescript-eslint/no-floating-promises");
  const path = join(useApi().directory, "NodeUnrelated.test.ts");
  const runtime = capture(["bun", "test", "--isolate", path], { timeoutMs: 30_000 });
  assertNotEqual(runtime.returncode, 0, runtime.stdout + runtime.stderr);
  assertIn("unhandled-promise-proof", runtime.stderr);
}

function testActualWebAndEditorDeclarationPreparationIsNodeFree(): void {
  const prepared = capture(["bun", "--bun", "scripts/prepare-vue-lint-types.mjs"]);
  assertEqual(prepared.returncode, 0, prepared.stdout + prepared.stderr);
  const output = join(root, "node_modules/.cache/fvoci-vue-lint/types");
  const app = join(output, "apps/web/src/vue/App.vue.d.ts");
  assertIn('import("vue").DefineComponent', readFileSync(app, "utf8"));
  assertEqual(readdirSync(join(output, "packages/editor/src/vue")).filter((name) => name.endsWith(".vue.d.ts")).length, 8);
  const result = capture([...eslint, "packages/editor/src/vue/node-views.ts", "--max-warnings=0", "--format=json"]);
  assertEqual(result.returncode, 0, result.stdout + result.stderr);
  const source = `import { createApp } from "vue";
import App from "./App.vue";
export const app = createApp(App);`;
  const typed = capture([...eslint, "--stdin", "--stdin-filename", "apps/web/src/vue/main.ts", "--max-warnings=0", "--format=json"], { input: source });
  assertEqual(typed.returncode, 0, typed.stdout + typed.stderr);
}

function testMultipleDeclarationProjectsFailClosedTogether(): void {
  const { source, directory } = useApi();
  const app = source("ProofApp.vue", `<script setup lang="ts">
defineProps<{ label: string }>();
</script><template><p>{{ label }}</p></template>`);
  const editor = source("ProofEditor.vue", `<script setup lang="ts">
import type { NodeViewProps } from "@tiptap/vue-3";
defineProps<NodeViewProps>();
</script><template><span /></template>`);
  const output = join(directory, "combined");
  const projects: string[] = [];
  for (const [name, component, config] of [
    ["editor", editor, "packages/editor/tsconfig.vue.json"],
    ["web", app, "apps/web/tsconfig.vue.json"],
  ] as const) {
    const project = join(directory, `tsconfig.${name}.emit.json`);
    writeFileSync(project, JSON.stringify({
      extends: join(root, config),
      compilerOptions: { rootDir: directory, incremental: false, composite: false },
      include: [component],
      exclude: [],
    }));
    projects.push(project);
  }
  const prepare = source("prepare-combined.mjs", `import { prepareVueLintTypes } from ${JSON.stringify(join(root, "scripts/prepare-vue-lint-types.mjs"))};
prepareVueLintTypes(${JSON.stringify(projects)}, ${JSON.stringify(output)});`);
  const emitted = capture(["bun", "--bun", prepare]);
  assertEqual(emitted.returncode, 0, emitted.stdout + emitted.stderr);
  for (const name of ["ProofApp.vue.d.ts", "ProofEditor.vue.d.ts"]) assertTrue(existsSync(join(output, name)), name);
  const consumer = source("ProofCombined.ts", `import { createApp } from "vue";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
import ProofApp from "./ProofApp.vue";
import ProofEditor from "./ProofEditor.vue";
export const app = createApp(ProofApp);
export const renderer = VueNodeViewRenderer(ProofEditor);
export function read(value: InstanceType<typeof ProofApp>): string { return value.$props.label; }`);
  const lintProject = join(directory, "tsconfig.combined.json");
  writeFileSync(lintProject, JSON.stringify({
    extends: join(root, "apps/web/tsconfig.eslint.json"),
    compilerOptions: { rootDirs: [directory, output] },
    include: [consumer],
    exclude: [],
  }));
  const args = [...eslint, consumer, "--max-warnings=0", "--format=json", "--parser-options", JSON.stringify({ project: [lintProject] })];
  const typed = capture(args);
  assertEqual(typed.returncode, 0, typed.stdout + typed.stderr);
  writeFileSync(app, '<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>');
  const failed = capture(["bun", "--bun", prepare]);
  assertNotEqual(failed.returncode, 0);
  assertIn("TS2322", failed.stdout + failed.stderr);
  if (existsSync(output)) fail("stale combined output remains");
  const missing = capture(args);
  assertNotEqual(missing.returncode, 0);
  const messages = (JSON.parse(missing.stdout) as LintFile[]).flatMap((file) => file.messages);
  assertEqual(messages.filter((message) => message.ruleId === "@typescript-eslint/no-unsafe-argument").length, 2, messages);
}

function testGeneratedSfcTypesAndFailedRefreshHaveNoWaiver(): void {
  const { source, directory } = useApi();
  const component = source("ProofGenerated.vue", `<script setup lang="ts">
defineProps<{ label: string }>();
</script><template><p>{{ label }}</p></template>`);
  const output = join(directory, "generated");
  const project = join(directory, "tsconfig.emit.json");
  writeFileSync(project, JSON.stringify({
    extends: join(root, "apps/web/tsconfig.vue.json"),
    compilerOptions: { rootDir: directory, incremental: false, composite: false },
    include: [component],
    exclude: [],
  }));
  const prepare = source("prepare.mjs", `import { prepareVueLintTypes } from ${JSON.stringify(join(root, "scripts/prepare-vue-lint-types.mjs"))};
prepareVueLintTypes(${JSON.stringify(project)}, ${JSON.stringify(output)});`);
  const emitted = capture(["bun", "--bun", prepare]);
  assertEqual(emitted.returncode, 0, emitted.stdout + emitted.stderr);
  const declaration = join(output, "ProofGenerated.vue.d.ts");
  assertTrue(existsSync(declaration));
  const consumer = source("ProofGeneratedConsumer.ts", `import ProofGenerated from "./ProofGenerated.vue";
export function read(value: InstanceType<typeof ProofGenerated>): string { return value.$props.label; }`);
  const lintProject = join(directory, "tsconfig.generated.json");
  writeFileSync(lintProject, JSON.stringify({
    extends: join(root, "apps/web/tsconfig.eslint.json"),
    compilerOptions: { rootDirs: [directory, output] },
    include: [consumer],
    exclude: [],
  }));
  const args = [...eslint, consumer, "--max-warnings=0", "--format=json", "--parser-options", JSON.stringify({ project: [lintProject] })];
  const typed = capture(args);
  assertEqual(typed.returncode, 0, typed.stdout + typed.stderr);
  writeFileSync(consumer, readFileSync(consumer, "utf8").replace("value.$props.label", "value.$props.label.missing"));
  const invalid = capture(["bun", "--bun", join(root, "node_modules/.bin/tsc"), "--noEmit", "-p", lintProject]);
  assertNotEqual(invalid.returncode, 0);
  assertIn("TS2339", invalid.stdout + invalid.stderr);
  writeFileSync(consumer, readFileSync(consumer, "utf8").replace("value.$props.label.missing", "value.$props.label"));
  writeFileSync(component, readFileSync(component, "utf8").replace("label: string", "label: any"));
  const refreshed = capture(["bun", "--bun", prepare]);
  assertEqual(refreshed.returncode, 0, refreshed.stdout + refreshed.stderr);
  const unsafe = capture(args);
  assertNotEqual(unsafe.returncode, 0);
  assertIn("@typescript-eslint/no-unsafe-return", new Set((JSON.parse(unsafe.stdout) as LintFile[]).flatMap((file) => file.messages.map((message) => message.ruleId ?? ""))));
  writeFileSync(component, '<script setup lang="ts">defineProps<{ label: string }>();</script><template><p>{{ label.toFixed() }}</p></template>');
  const failed = capture(["bun", "--bun", prepare]);
  assertNotEqual(failed.returncode, 0);
  assertIn("TS2551", failed.stdout + failed.stderr);
  if (existsSync(declaration)) fail("stale declaration remains");
  const missing = capture(args);
  assertNotEqual(missing.returncode, 0);
  assertIn("@typescript-eslint/no-unsafe-return", new Set((JSON.parse(missing.stdout) as LintFile[]).flatMap((file) => file.messages.map((message) => message.ruleId ?? ""))));
}

function testUnpreparedSfcImportHasNoFakeFallback(): void {
  useApi().source("ProofNode.vue", `<script setup lang="ts">
import type { NodeViewProps } from "@tiptap/vue-3";
defineProps<NodeViewProps>();
</script><template><span /></template>`);
  const source = `import ProofNode from "./ProofNode.vue";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
export const renderer = VueNodeViewRenderer(ProofNode);`;
  useApi().assertRule("ProofSfcImport.ts", source, "@typescript-eslint/no-unsafe-argument");
  for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]) {
    const typed = useApi().compiler(config, source, "ts", "vue-tsc");
    assertEqual(typed.returncode, 0, typed.stdout + typed.stderr);
    const broken = useApi().compiler(config, `${source}\nexport const invalid: number = "bad";`, "ts", "vue-tsc");
    assertNotEqual(broken.returncode, 0);
    assertIn("TS2322", broken.stdout + broken.stderr);
  }
}

function testYTextDeclaredStringContractWithoutRuleAllowance(): void {
  const source = 'import * as Y from "yjs"; export function text(value: Y.Text): string { return value.toJSON(); }';
  const linted = useApi().lint("ProofYText.ts", source);
  assertEqual(linted.result.returncode, 0, linted.report);
  useApi().assertRule("ProofYTextInherited.ts", source.replace("value.toJSON()", "value.toString()"), "@typescript-eslint/no-base-to-string");
  useApi().assertRule("ProofObjectString.ts", "export function text(value: object): string { return value.toString(); }", "@typescript-eslint/no-base-to-string");
}

function testFormatterKeepsImportOrderTextAndTailwind(): void {
  const ordered = 'import "./z.css";\nimport "./a.css";\nexport const value = 1;\n';
  const order = capture([...prettier, "--stdin-filepath", "order.ts"], { input: ordered });
  assertEqual(order.returncode, 0, order.stderr);
  assertLess(order.stdout.indexOf('"./z.css"'), order.stdout.indexOf('"./a.css"'));
  const text = "<template><p>before <strong>middle</strong> after</p><pre>  keep\n spacing </pre></template>";
  const vue = capture([...prettier, "--stdin-filepath", "text.vue"], { input: text });
  assertIn("before <strong>middle</strong> after", vue.stdout);
  assertIn("  keep\n spacing ", vue.stdout);
  const css = '@import "tailwindcss" source(none);\n@source "./";\n@theme { --color-brand: #123456; }\n@utility proof { @apply flex; }\n';
  const formatted = capture([...prettier, "--stdin-filepath", "proof.css"], { input: css });
  assertEqual(formatted.returncode, 0, formatted.stderr);
}

const caseList: [string, () => void][] = [
  ["VueToolchain.test_actual_nuxt_autoimport_registration_boundary", testActualNuxtAutoimportRegistrationBoundary],
  ["VueToolchain.test_actual_web_and_editor_declaration_preparation_is_node_free", testActualWebAndEditorDeclarationPreparationIsNodeFree],
  ["VueToolchain.test_browser_does_not_receive_node_or_bun_globals", testBrowserDoesNotReceiveNodeOrBunGlobals],
  ["VueToolchain.test_browser_worker_and_i18n_reject_runtime_node_bun", testBrowserWorkerAndI18nRejectRuntimeNodeBun],
  ["VueToolchain.test_bun_development_types_and_runtime", testBunDevelopmentTypesAndRuntime],
  ["VueToolchain.test_directives_unused_any_and_typed_promises_fail", testDirectivesUnusedAnyAndTypedPromisesFail],
  ["VueToolchain.test_dynamic_slots_have_no_unused_false_positive", testDynamicSlots],
  ["VueToolchain.test_exact_development_export_buffer_contract", testExactDevelopmentExportBufferContract],
  ["VueToolchain.test_formatter_keeps_import_order_text_and_tailwind", testFormatterKeepsImportOrderTextAndTailwind],
  ["VueToolchain.test_generated_sfc_types_and_failed_refresh_have_no_waiver", testGeneratedSfcTypesAndFailedRefreshHaveNoWaiver],
  ["VueToolchain.test_indexed_access_preserves_missing_route_fallbacks", testIndexedAccessPreservesMissingRouteFallbacks],
  ["VueToolchain.test_multiple_declaration_projects_fail_closed_together", testMultipleDeclarationProjectsFailClosedTogether],
  ["VueToolchain.test_node_test_runner_failure_propagation_and_no_waiver", testNodeTestRunnerFailurePropagationAndNoWaiver],
  ["VueToolchain.test_qualified_runtime_globals_and_local_names", testQualifiedRuntimeGlobalsAndLocalNames],
  ["VueToolchain.test_runtime_and_positive_sfc", testRuntimeAndPositiveSfc],
  ["VueToolchain.test_strict_script_template_props_emits_and_slots_types", testStrictScriptTemplatePropsEmitsAndSlotsTypes],
  ["VueToolchain.test_unprepared_sfc_import_has_no_fake_fallback", testUnpreparedSfcImportHasNoFakeFallback],
  ["VueToolchain.test_unused_disables_warnings_and_unmatched_paths_fail", testUnusedDisablesWarningsAndUnmatchedPathsFail],
  ["VueToolchain.test_y_text_declared_string_contract_without_rule_allowance", testYTextDeclaredStringContractWithoutRuleAllowance],
];

export const testIds = caseList.map(([id]) => id).sort();
const cases = new Map(caseList);

export type Selection = { ok: true; ids: string[] } | { ok: false; status: number; message: string };

export function selectTestIds(argv: readonly string[], available: readonly string[]): Selection {
  if (argv.some((arg) => arg.startsWith("-"))) {
    return { ok: false, status: 2, message: `positional test ids only: ${argv.join(" ")}` };
  }
  if (argv.length === 0) return { ok: true, ids: [...available] };
  const ids: string[] = [];
  const seen = new Set<string>();
  for (const arg of argv) {
    const normalized = arg.replace(/^__main__\./, "");
    const matched = normalized === "VueToolchain"
      ? [...available]
      : available.includes(normalized)
        ? [normalized]
        : available.includes(`VueToolchain.${normalized}`)
          ? [`VueToolchain.${normalized}`]
          : [];
    if (matched.length === 0) return { ok: false, status: 1, message: `no such test: ${arg}` };
    for (const id of matched) {
      if (seen.has(id)) continue;
      seen.add(id);
      ids.push(id);
    }
  }
  return { ok: true, ids };
}

function describe(id: string): string {
  const method = id.startsWith("VueToolchain.") ? id.slice("VueToolchain.".length) : id;
  return `${method} (__main__.${id})`;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.stack ?? error.message : String(error);
}

export function executeSelected(
  ids: readonly string[],
  selected: ReadonlyMap<string, () => void>,
  write: (text: string) => void,
  hooks: { setup?: () => void; cleanup?: () => void } = {},
): number {
  const started = performance.now();
  const failures: { id: string; error: unknown }[] = [];
  const errors: { id: string; error: unknown }[] = [];
  let setupError: unknown;
  try {
    if (ids.length) hooks.setup?.();
    for (const id of ids) {
      const fn = selected.get(id);
      write(`${describe(id)} ... `);
      if (!fn) {
        const error = new Error(`missing case ${id}`);
        errors.push({ id, error });
        write("ERROR\n");
        continue;
      }
      try {
        fn();
        write("ok\n");
      } catch (error) {
        failures.push({ id, error });
        write("FAIL\n");
      }
    }
  } catch (error) {
    setupError = error;
  } finally {
    hooks.cleanup?.();
  }
  if (setupError !== undefined || failures.length || errors.length) write("\n");
  if (setupError !== undefined) {
    write(`${separator1}\nERROR: setup\n${separator2}\n${errorText(setupError)}\n`);
  }
  for (const row of errors) write(`${separator1}\nERROR: ${describe(row.id)}\n${separator2}\n${errorText(row.error)}\n`);
  for (const row of failures) write(`${separator1}\nFAIL: ${describe(row.id)}\n${separator2}\n${errorText(row.error)}\n`);
  const seconds = ((performance.now() - started) / 1000).toFixed(3);
  write(`\n${separator2}\nRan ${ids.length} test${ids.length === 1 ? "" : "s"} in ${seconds}s\n\n`);
  if (setupError !== undefined || failures.length || errors.length) {
    const parts: string[] = [];
    if (failures.length) parts.push(`failures=${failures.length}`);
    const errorCount = errors.length + (setupError !== undefined ? 1 : 0);
    if (errorCount) parts.push(`errors=${errorCount}`);
    write(`FAILED (${parts.join(", ")})\n`);
    return 1;
  }
  write("OK\n");
  return 0;
}

function setup(): void {
  const verified = capture(["bun", "--bun", "scripts/verify-web-tools.mjs"]);
  if (verified.returncode) throw new Error(verified.stdout + verified.stderr);
  const directory = mkdtempSync(join(root, "apps/web", "eslint-proof-"));
  api = createApi(directory);
  api.source("props.ts", "export interface ImportedProps { title: string }\n");
  api.source("ProofChild.vue", child);
}

function cleanup(): void {
  api?.cleanup();
  api = undefined;
}

export function main(argv: readonly string[]): number {
  const selection = selectTestIds(argv, testIds);
  if (!selection.ok) {
    process.stderr.write(`${selection.message}\n`);
    return selection.status;
  }
  return executeSelected(selection.ids, cases, (chunk) => {
    process.stderr.write(chunk);
  }, { setup, cleanup });
}

const child = '<script setup lang="ts">\ndefineProps<{ label: string }>();\ndefineSlots<{ default(props: { row: { id: number; label: string } }): unknown }>();\n</script>\n<template><slot :row="{ id: 1, label }" /></template>\n';
const valid = '<script setup lang="ts">\nimport UButton from "@nuxt/ui/components/Button.vue";\nimport { ref } from "vue";\nimport ProofChild from "./ProofChild.vue";\nimport type { ImportedProps } from "./props";\n\ndefineProps<ImportedProps & { enabled: boolean }>();\nconst emit = defineEmits<{ select: [id: number] }>();\ndefineSlots<{ default(props: { value: string }): unknown }>();\nconst model = ref("");\nconst rows = [{ id: 1, label: "one" }];\nfunction select(id: number): void { emit("select", id); }\n</script>\n<template>\n  <section v-if="enabled" class="flex gap-2 hover:bg-teal-50">\n    <label>Search<input v-model="model" name="search" /></label>\n    <ProofChild :label="title">\n      <template #default="{ row }"><span>{{ row.label }}</span></template>\n    </ProofChild>\n    <UButton v-for="row in rows" :key="row.id" type="button" @click="select(row.id)">{{ row.label }}</UButton>\n    <slot :value="model" />\n  </section>\n</template>\n<style scoped>\n.host :deep(.child) { color: red; }\n.host :slotted(span) { color: blue; }\n</style>\n';

if (import.meta.main) process.exit(main(process.argv.slice(2)));
