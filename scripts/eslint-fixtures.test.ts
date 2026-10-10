// Positive and negative proofs for the pinned Bun-only Vue toolchain.
// Run with `bun run lint:fixtures`; one proof with `-t "<test name>"`.
import { afterAll, afterEach, beforeAll, describe, expect, test } from "bun:test";
import { spawn } from "node:child_process";
import {
  closeSync,
  existsSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..");
const BUN = process.execPath;
const ESLINT = [BUN, "--bun", join(ROOT, "node_modules/eslint/bin/eslint.js")];
const PRETTIER = [BUN, "--bun", join(ROOT, "node_modules/prettier/bin/prettier.cjs")];
const NODE_RUNTIME_GLOBALS = [
  "Bun",
  "process",
  "Buffer",
  "require",
  "__dirname",
  "__filename",
  "module",
  "exports",
  "setImmediate",
  "clearImmediate",
];

interface Message {
  ruleId: string | null;
  message: string;
  severity: number;
}
interface FileReport {
  messages: Message[];
  errorCount: number;
  warningCount: number;
  fatalErrorCount: number;
}
interface RuleOptions {
  globals?: string[];
  checkGlobalObject?: boolean;
  ignoreVoid?: boolean;
}
interface PrintedConfig {
  languageOptions: { globals?: Record<string, unknown> };
  rules: Record<string, [unknown, RuleOptions?]>;
}
interface Run {
  status: number;
  stdout: string;
  stderr: string;
}

let directory = "";
let outputDirectory = "";

// `bun test` injects NODE_ENV=test; the lint/format/compile children must see
// the same environment as `bun run lint`, not the test runner's, so NODE_ENV
// is removed whatever its value.
const CHILD_ENV = Object.fromEntries(
  Object.entries(process.env).filter(([name]) => name !== "NODE_ENV"),
);

// Each child leads its own process group, so an expired limit reaches the
// whole tree (the prepare helper's vue-tsc included), not only the direct child.
const CHILD_LIMIT_MS = 300_000;
const KILL_GRACE_MS = 5_000;
const liveGroups = new Set<number>();
// Groups killed by cleanup after the runner abandoned their test.
const abandonedGroups = new Set<number>();
let outputCount = 0;

function signalGroup(group: number, signal: NodeJS.Signals): void {
  try {
    process.kill(-group, signal);
  } catch (error) {
    // ESRCH: every member of the group has already exited.
    if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
  }
}

function killLiveGroups(): void {
  for (const group of liveGroups) {
    signalGroup(group, "SIGKILL");
    abandonedGroups.add(group);
  }
  liveGroups.clear();
}

// A runner-level test timeout abandons the awaiting test, and detached groups
// do not receive the terminal's SIGINT; never leave a tree behind either way.
afterEach(killLiveGroups);
process.on("exit", killLiveGroups);
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) {
  process.once(signal, () => {
    killLiveGroups();
    // The listener is gone, so re-raising applies the default disposition.
    process.kill(process.pid, signal);
  });
}

async function run(
  args: string[],
  options: { input?: string; timeout?: number } = {},
): Promise<Run> {
  // Bun 1.4.2 + ESLint's exit path can truncate >64KiB piped stdout.
  // A regular file preserves the complete print-config/JSON evidence.
  outputCount += 1;
  const outputPath = join(outputDirectory, `stdout-${String(outputCount)}`);
  const output = openSync(outputPath, "w+");
  try {
    const [command, ...rest] = args;
    if (command === undefined) throw new Error("empty command");
    const limit = options.timeout ?? CHILD_LIMIT_MS;
    const child = spawn(command, rest, {
      cwd: ROOT,
      env: CHILD_ENV,
      detached: true,
      stdio: [options.input === undefined ? "ignore" : "pipe", output, "pipe"],
    });
    const stderr: Buffer[] = [];
    child.stderr?.on("data", (chunk: Buffer) => stderr.push(chunk));
    const expiry = { hit: false };
    let termTimer: ReturnType<typeof setTimeout> | undefined;
    let killTimer: ReturnType<typeof setTimeout> | undefined;
    let closed: { code: number | null; signal: NodeJS.Signals | null };
    try {
      closed = await new Promise((resolveClose, rejectClose) => {
        child.once("error", rejectClose);
        // "close" waits until every holder of the stderr pipe (descendants too) is gone.
        child.once("close", (code, signal) => {
          resolveClose({ code, signal });
        });
        const group = child.pid;
        if (group === undefined) return;
        liveGroups.add(group);
        termTimer = setTimeout(() => {
          expiry.hit = true;
          signalGroup(group, "SIGTERM");
          killTimer = setTimeout(() => {
            signalGroup(group, "SIGKILL");
          }, KILL_GRACE_MS);
        }, limit);
        if (child.stdin !== null) {
          child.stdin.on("error", (error: NodeJS.ErrnoException) => {
            // A child may exit without reading its input; its status is the evidence.
            if (error.code !== "EPIPE") rejectClose(error);
          });
          child.stdin.end(options.input);
        }
      });
    } finally {
      clearTimeout(termTimer);
      clearTimeout(killTimer);
      if (child.pid !== undefined) liveGroups.delete(child.pid);
    }
    if (child.pid !== undefined && abandonedGroups.delete(child.pid)) {
      // The runner already failed this test; settling now would surface a
      // stray rejection in whichever test runs next.
      return await new Promise<never>(() => undefined);
    }
    const stderrText = Buffer.concat(stderr).toString("utf8");
    if (expiry.hit) {
      // A descendant that obeyed neither signal before the leader closed.
      if (child.pid !== undefined) signalGroup(child.pid, "SIGKILL");
      throw new Error(
        `${args.join(" ")} exceeded its ${String(limit)}ms limit; its process group was sent SIGTERM, then SIGKILL within ${String(KILL_GRACE_MS)}ms (exit=${String(closed.code)}, signal=${String(closed.signal)})\n${stderrText}`,
      );
    }
    if (closed.code === null) {
      throw new Error(`${args.join(" ")} terminated by ${String(closed.signal)}`);
    }
    return {
      status: closed.code,
      stdout: readFileSync(outputPath, "utf8"),
      stderr: stderrText,
    };
  } finally {
    closeSync(output);
  }
}

function writeSource(name: string, source: string): string {
  const path = join(directory, name);
  writeFileSync(path, source);
  return path;
}

function parseReport(stdout: string): FileReport[] {
  return JSON.parse(stdout) as FileReport[];
}

function messages(report: FileReport[]): Message[] {
  return report.flatMap((file) => file.messages);
}

function ruleIds(report: FileReport[]): Set<string | null> {
  return new Set(messages(report).map((message) => message.ruleId));
}

async function lint(
  name: string,
  source: string,
  ...options: string[]
): Promise<[Run, FileReport[]]> {
  const path = writeSource(name, source);
  const result = await run([...ESLINT, path, "--max-warnings=0", "--format=json", ...options]);
  return [result, parseReport(result.stdout)];
}

async function assertRule(name: string, source: string, rule: string): Promise<void> {
  const [result, report] = await lint(name, source);
  const detail = JSON.stringify(report);
  expect(result.status, detail).not.toBe(0);
  expect(ruleIds(report).has(rule), detail).toBe(true);
}

async function compiler(
  config: string,
  source: string,
  extension = "vue",
  program = "vue-tsc",
): Promise<Run> {
  const path = writeSource(`TypeProof.${extension}`, source);
  const configPath = join(directory, "tsconfig.json");
  writeFileSync(
    configPath,
    JSON.stringify({
      extends: join(ROOT, config),
      compilerOptions: { incremental: false, composite: false },
      include: [path],
      exclude: [],
    }),
  );
  return await run([
    BUN,
    "--bun",
    join(ROOT, "node_modules/.bin", program),
    "--noEmit",
    "-p",
    configPath,
  ]);
}

async function stdinLint(path: string, input: string): Promise<Run> {
  return await run(
    [...ESLINT, "--stdin", "--stdin-filename", path, "--max-warnings=0", "--format=json"],
    { input },
  );
}

async function printConfig(path: string): Promise<Run> {
  return await run([...ESLINT, "--print-config", path]);
}

function ruleOptions(config: PrintedConfig, name: string): RuleOptions {
  const options = config.rules[name]?.[1];
  if (options === undefined)
    throw new Error(`${name} has no options: ${JSON.stringify(config.rules[name])}`);
  return options;
}

function sorted(values: Iterable<string>): string[] {
  return [...values].sort();
}

const RUNTIME_PATHS: [string, "browser" | "worker" | "library"][] = [
  ["apps/web/src/vue/features/settings/toggle.ts", "browser"],
  ["apps/web/src/features/attachments/hwp-worker.ts", "worker"],
  ["apps/web/src/features/attachments/pptx-worker.ts", "worker"],
  ["apps/web/src/features/attachments/xlsx-worker.ts", "worker"],
  ["apps/web/public/sw.js", "worker"],
  ["packages/i18n/src/index.ts", "library"],
];

const CHILD =
  '<script setup lang="ts">\ndefineProps<{ label: string }>();\ndefineSlots<{ default(props: { row: { id: number; label: string } }): unknown }>();\n</script>\n<template><slot :row="{ id: 1, label }" /></template>\n';

const VALID =
  '<script setup lang="ts">\nimport UButton from "@nuxt/ui/components/Button.vue";\nimport { ref } from "vue";\nimport ProofChild from "./ProofChild.vue";\nimport type { ImportedProps } from "./props";\n\ndefineProps<ImportedProps & { enabled: boolean }>();\nconst emit = defineEmits<{ select: [id: number] }>();\ndefineSlots<{ default(props: { value: string }): unknown }>();\nconst model = ref("");\nconst rows = [{ id: 1, label: "one" }];\nfunction select(id: number): void { emit("select", id); }\n</script>\n<template>\n  <section v-if="enabled" class="flex gap-2 hover:bg-teal-50">\n    <label>Search<input v-model="model" name="search" /></label>\n    <ProofChild :label="title">\n      <template #default="{ row }"><span>{{ row.label }}</span></template>\n    </ProofChild>\n    <UButton v-for="row in rows" :key="row.id" type="button" @click="select(row.id)">{{ row.label }}</UButton>\n    <slot :value="model" />\n  </section>\n</template>\n<style scoped>\n.host :deep(.child) { color: red; }\n.host :slotted(span) { color: blue; }\n</style>\n';

function prepareScript(name: string, projects: string | string[], output: string): string {
  return writeSource(
    name,
    `import { prepareVueLintTypes } from ${JSON.stringify(join(ROOT, "scripts/prepare-vue-lint-types.mjs"))};
prepareVueLintTypes(${JSON.stringify(projects)}, ${JSON.stringify(output)});`,
  );
}

// Tests run in the order unittest used (alphabetical), sharing one fixture
// directory under apps/web so the real root config and projects apply.
describe("VueToolchain", () => {
  beforeAll(async () => {
    outputDirectory = mkdtempSync(join(tmpdir(), "eslint-proof-out-"));
    const verified = await run([BUN, "--bun", "scripts/verify-web-tools.mjs"]);
    if (verified.status !== 0) throw new Error(verified.stdout + verified.stderr);
    directory = mkdtempSync(join(ROOT, "apps/web/eslint-proof-"));
    writeFileSync(
      join(directory, "props.ts"),
      "export interface ImportedProps { title: string }\n",
    );
    writeFileSync(join(directory, "ProofChild.vue"), CHILD);
  });

  afterAll(() => {
    if (directory !== "") rmSync(directory, { recursive: true, force: true });
    if (outputDirectory !== "") rmSync(outputDirectory, { recursive: true, force: true });
  });

  test("actual_nuxt_autoimport_registration_boundary", async () => {
    // FVOCI disables Nuxt UI autoimports: an unregistered template name must fail.
    const result = await compiler(
      "apps/web/tsconfig.vue.json",
      '<template><UButton type="button">ok</UButton></template>',
    );
    expect(result.status).not.toBe(0);
    expect(result.stdout).toContain("UButton");
  });

  test("actual_web_and_editor_declaration_preparation_is_node_free", async () => {
    const prepared = await run([BUN, "--bun", "scripts/prepare-vue-lint-types.mjs"]);
    expect(prepared.status, prepared.stdout + prepared.stderr).toBe(0);
    const output = join(ROOT, "node_modules/.cache/fvoci-vue-lint/types");
    const app = join(output, "apps/web/src/vue/App.vue.d.ts");
    expect(readFileSync(app, "utf8")).toContain('import("vue").DefineComponent');
    const declarations = readdirSync(join(output, "packages/editor/src/vue")).filter((name) =>
      name.endsWith(".vue.d.ts"),
    );
    expect(declarations.length).toBe(8);
    let result = await run([
      ...ESLINT,
      "packages/editor/src/vue/node-views.ts",
      "--max-warnings=0",
      "--format=json",
    ]);
    expect(result.status, result.stdout + result.stderr).toBe(0);
    // Use the real main.ts import location and rootDirs, without its unrelated
    // source diagnostics masking whether createApp receives a genuine type.
    const source = `import { createApp } from "vue";
import App from "./App.vue";
export const app = createApp(App);`;
    result = await stdinLint("apps/web/src/vue/main.ts", source);
    expect(result.status, result.stdout + result.stderr).toBe(0);
  });

  test.each(RUNTIME_PATHS)(
    "browser_does_not_receive_node_or_bun_globals %s",
    async (path, environment) => {
      const forbidden = new Set(NODE_RUNTIME_GLOBALS);
      const result = await printConfig(path);
      expect(result.status, `${path}: ${result.stderr}`).toBe(0);
      const config = JSON.parse(result.stdout) as PrintedConfig;
      const declared = config.languageOptions.globals ?? {};
      const leaked = Object.keys(declared).filter((name) => forbidden.has(name));
      expect(leaked, `${path}: ${JSON.stringify(declared)}`).toEqual([]);
      const runtimeRule = ruleOptions(config, "no-restricted-globals");
      const expected =
        environment === "worker" ? [...NODE_RUNTIME_GLOBALS, "window"] : NODE_RUNTIME_GLOBALS;
      expect(sorted(new Set(runtimeRule.globals)), path).toEqual(sorted(expected));
      expect(runtimeRule.checkGlobalObject, path).toBe(true);
      if (environment === "browser") {
        expect(Object.hasOwn(declared, "window"), path).toBe(true);
      } else if (environment === "worker") {
        expect(Object.hasOwn(declared, "self"), path).toBe(true);
        expect(Object.hasOwn(declared, "postMessage"), path).toBe(true);
        expect(Object.hasOwn(declared, "window"), path).toBe(false);
      }
      if (path.endsWith(".ts")) {
        expect(
          ruleOptions(config, "@typescript-eslint/no-floating-promises").ignoreVoid,
          path,
        ).toBe(false);
      }
    },
  );

  test.each(RUNTIME_PATHS)(
    "browser_worker_and_i18n_reject_runtime_node_bun %s",
    async (path, environment) => {
      // Stdin with actual file paths exercises the real root config/projects
      // without writing product files or replacing the parser/rule set.
      const negative = `export const forbidden = [process.pid, Bun.version,
Buffer.alloc(0), require("bad"), __dirname, __filename, module, exports,
setImmediate, clearImmediate];`;
      const positive = {
        browser: "export const href = window.location.href;",
        worker: 'self.postMessage("ready");',
        library: "export const add = (left: number, right: number): number => left + right;",
      }[environment];
      const valid = await stdinLint(path, positive);
      expect(valid.status, `${path}: ${valid.stdout}${valid.stderr}`).toBe(0);
      const invalid = await stdinLint(path, negative);
      const report = parseReport(invalid.stdout);
      const detail = `${path}: ${JSON.stringify(report)}`;
      expect(invalid.status, detail).not.toBe(0);
      expect(
        report.reduce((sum, file) => sum + file.fatalErrorCount, 0),
        detail,
      ).toBe(0);
      const restricted = messages(report).filter(
        (message) => message.ruleId === "no-restricted-globals",
      );
      const named = new Set(restricted.map((message) => message.message.split("'")[1] ?? ""));
      expect(sorted(named), detail).toEqual(sorted(NODE_RUNTIME_GLOBALS));
      expect(
        restricted.every((message) => message.severity === 2),
        detail,
      ).toBe(true);
    },
  );

  test("bun_development_types_and_runtime", async () => {
    const source = `import assert from "node:assert/strict";
import { mock, test } from "bun:test";
await mock.module("fvoci-bun-typing-proof", () => ({ value: 1 }));
test("typed Bun Transpiler and module mock", () => {
  const code = new Bun.Transpiler({ loader: "ts" }).transformSync("export const value: number = 1;");
  assert.equal(typeof code, "string");
  assert.equal(code.includes("number"), false);
});`;
    const [result, report] = await lint("ProofBun.test.ts", source);
    expect(result.status, JSON.stringify(report)).toBe(0);
    const path = join(directory, "ProofBun.test.ts");
    const runtime = await run([BUN, "test", "--isolate", path], { timeout: 30_000 });
    expect(runtime.status, runtime.stdout + runtime.stderr).toBe(0);
    expect(runtime.stderr).toContain("1 pass");
    for (const config of [
      "apps/web/tsconfig.eslint.json",
      "packages/editor/tsconfig.eslint.json",
    ]) {
      const valid = await compiler(config, source, "ts", "tsc");
      expect(valid.status, `${config}: ${valid.stdout}${valid.stderr}`).toBe(0);
      const invalid = await compiler(
        config,
        source.replace('loader: "ts"', 'loader: "invalid-loader"'),
        "ts",
        "tsc",
      );
      expect(invalid.status, config).not.toBe(0);
      expect(invalid.stdout + invalid.stderr, config).toContain("TS2322");
    }
    await assertRule(
      "ProofBunUnsafe.test.ts",
      'import { mock } from "bun:test"; mock.module("proof", () => ({ value: 1 }));',
      "@typescript-eslint/no-floating-promises",
    );
    await assertRule(
      "ProofBunUnrelated.test.ts",
      'import { test } from "bun:test"; test("proof", () => { Promise.resolve(1); });',
      "@typescript-eslint/no-floating-promises",
    );
  });

  test.each([
    [
      "ProofUnused.vue",
      '<script setup lang="ts">const unused = 1;</script><template><p>ok</p></template>',
      "@typescript-eslint/no-unused-vars",
    ],
    [
      "ProofUnusedImport.vue",
      '<script setup lang="ts">import { ref } from "vue";</script><template><p>ok</p></template>',
      "@typescript-eslint/no-unused-vars",
    ],
    ["ProofParse.vue", '<template><p v-if="(">bad</p></template>', "vue/no-parsing-error"],
    [
      "ProofFor.vue",
      '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows">{{ row }}</p></template>',
      "vue/require-v-for-key",
    ],
    [
      "ProofKey.vue",
      '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows" :key="1">{{ row }}</p></template>',
      "vue/valid-v-for",
    ],
    [
      "ProofIfFor.vue",
      '<script setup lang="ts">const rows = [1];</script><template><p v-if="true" v-for="row in rows" :key="row">{{ row }}</p></template>',
      "vue/no-use-v-if-with-v-for",
    ],
    ["ProofIf.vue", "<template><p v-if>bad</p></template>", "vue/valid-v-if"],
    ["ProofModel.vue", '<template><input v-model="1" /></template>', "vue/valid-v-model"],
    ["ProofAny.ts", "export const value: any = 1;", "@typescript-eslint/no-explicit-any"],
    [
      "ProofUnsafe.ts",
      "export function read(value: any): string { return value; }",
      "@typescript-eslint/no-unsafe-return",
    ],
    [
      "ProofPromise.ts",
      "export const save = (): Promise<number> => Promise.resolve(1); save();",
      "@typescript-eslint/no-floating-promises",
    ],
    [
      "ProofVoid.ts",
      "export const save = (): Promise<number> => Promise.resolve(1); void save();",
      "@typescript-eslint/no-floating-promises",
    ],
  ])("directives_unused_any_and_typed_promises_fail %s", async (name, source, rule) => {
    await assertRule(name, source, rule);
  });

  test("dynamic_slots_have_no_unused_false_positive", async () => {
    const source = `<script setup lang="ts">
import ProofChild from "./ProofChild.vue";
const slotName = "default";
</script>
<template><ProofChild label="ok"><template #[slotName]="{ row }"><span>{{ row.label }}</span></template></ProofChild></template>`;
    const [result, report] = await lint("ProofDynamic.vue", source);
    expect(result.status, JSON.stringify(report)).toBe(0);
    const compiled = await compiler(
      "apps/web/tsconfig.vue.json",
      source.replace("#[slotName]", "#[missingSlotName]"),
    );
    expect(compiled.status).not.toBe(0);
    expect(compiled.stdout).toContain("missingSlotName");
  });

  test.each(["packages/editor/src/export/docx.ts", "packages/editor/src/export/pptx.ts"])(
    "exact_development_export_buffer_contract %s",
    async (path) => {
      const valid = await stdinLint(path, 'export const bytes = Buffer.from("oracle", "utf8");');
      expect(valid.status, `${path}: ${valid.stdout}${valid.stderr}`).toBe(0);
      const config = JSON.parse((await printConfig(path)).stdout) as PrintedConfig;
      expect(config.languageOptions.globals?.Buffer, path).toBe("readonly");
      const rule = ruleOptions(config, "no-restricted-globals");
      expect(sorted(new Set(rule.globals)), path).toEqual(
        sorted(NODE_RUNTIME_GLOBALS.filter((name) => name !== "Buffer")),
      );
      const invalid = await stdinLint(
        path,
        "export const forbidden = [process.pid, Bun.version, globalThis.process.pid, globalThis.Bun.version];",
      );
      const report = parseReport(invalid.stdout);
      const detail = `${path}: ${JSON.stringify(report)}`;
      expect(invalid.status, detail).not.toBe(0);
      expect(
        report.reduce((sum, file) => sum + file.fatalErrorCount, 0),
        detail,
      ).toBe(0);
      const restricted = messages(report).filter(
        (message) => message.ruleId === "no-restricted-globals",
      );
      expect(restricted.length, detail).toBe(4);
    },
  );

  test.each([
    "packages/editor/src/json.ts",
    "packages/editor/src/export/limits.ts",
    "apps/web/src/features/attachments/hwp-worker.ts",
    "packages/i18n/src/index.ts",
  ])("exact_development_export_buffer_contract browser_path %s", async (path) => {
    const result = await stdinLint(path, "export const bytes = Buffer.alloc(0);");
    const report = parseReport(result.stdout);
    const detail = `${path}: ${JSON.stringify(report)}`;
    expect(result.status, detail).not.toBe(0);
    expect(ruleIds(report).has("no-restricted-globals"), detail).toBe(true);
  });

  test("formatter_keeps_import_order_text_and_tailwind", async () => {
    let result = await run([...PRETTIER, "--stdin-filepath", "order.ts"], {
      input: 'import "./z.css";\nimport "./a.css";\nexport const value = 1;\n',
    });
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout.indexOf('"./z.css"')).toBeGreaterThanOrEqual(0);
    expect(result.stdout.indexOf('"./z.css"')).toBeLessThan(result.stdout.indexOf('"./a.css"'));
    result = await run([...PRETTIER, "--stdin-filepath", "text.vue"], {
      input:
        "<template><p>before <strong>middle</strong> after</p><pre>  keep\n spacing </pre></template>",
    });
    expect(result.stdout).toContain("before <strong>middle</strong> after");
    expect(result.stdout).toContain("  keep\n spacing ");
    result = await run([...PRETTIER, "--stdin-filepath", "proof.css"], {
      input:
        '@import "tailwindcss" source(none);\n@source "./";\n@theme { --color-brand: #123456; }\n@utility proof { @apply flex; }\n',
    });
    expect(result.status, result.stderr).toBe(0);
  });

  test("generated_sfc_types_and_failed_refresh_have_no_waiver", async () => {
    const component = writeSource(
      "ProofGenerated.vue",
      `<script setup lang="ts">
defineProps<{ label: string }>();
</script><template><p>{{ label }}</p></template>`,
    );
    const output = join(directory, "generated");
    const project = join(directory, "tsconfig.emit.json");
    writeFileSync(
      project,
      JSON.stringify({
        extends: join(ROOT, "apps/web/tsconfig.vue.json"),
        compilerOptions: { rootDir: directory, incremental: false, composite: false },
        include: [component],
        exclude: [],
      }),
    );
    const prepare = prepareScript("prepare.mjs", project, output);
    let emitted = await run([BUN, "--bun", prepare]);
    expect(emitted.status, emitted.stdout + emitted.stderr).toBe(0);
    const declaration = join(output, "ProofGenerated.vue.d.ts");
    expect(existsSync(declaration) && statSync(declaration).isFile()).toBe(true);
    const consumer = writeSource(
      "ProofGeneratedConsumer.ts",
      `import ProofGenerated from "./ProofGenerated.vue";
export function read(value: InstanceType<typeof ProofGenerated>): string { return value.$props.label; }`,
    );
    const lintProject = join(directory, "tsconfig.generated.json");
    writeFileSync(
      lintProject,
      JSON.stringify({
        extends: join(ROOT, "apps/web/tsconfig.eslint.json"),
        compilerOptions: { rootDirs: [directory, output] },
        include: [consumer],
        exclude: [],
      }),
    );
    const args = [
      ...ESLINT,
      consumer,
      "--max-warnings=0",
      "--format=json",
      "--parser-options",
      JSON.stringify({ project: [lintProject] }),
    ];
    const typed = await run(args);
    expect(typed.status, typed.stdout + typed.stderr).toBe(0);
    writeFileSync(
      consumer,
      readFileSync(consumer, "utf8").replace("value.$props.label", "value.$props.label.missing"),
    );
    const invalid = await run([
      BUN,
      "--bun",
      join(ROOT, "node_modules/.bin/tsc"),
      "--noEmit",
      "-p",
      lintProject,
    ]);
    expect(invalid.status).not.toBe(0);
    expect(invalid.stdout + invalid.stderr).toContain("TS2339");
    writeFileSync(
      consumer,
      readFileSync(consumer, "utf8").replace("value.$props.label.missing", "value.$props.label"),
    );
    // Real unsafe props remain unsafe even through compiler-generated types.
    writeFileSync(
      component,
      readFileSync(component, "utf8").replace("label: string", "label: any"),
    );
    emitted = await run([BUN, "--bun", prepare]);
    expect(emitted.status, emitted.stdout + emitted.stderr).toBe(0);
    const unsafe = await run(args);
    expect(unsafe.status).not.toBe(0);
    expect(ruleIds(parseReport(unsafe.stdout)).has("@typescript-eslint/no-unsafe-return")).toBe(
      true,
    );
    // Failed refresh clears stale successful output before checking source.
    writeFileSync(
      component,
      '<script setup lang="ts">defineProps<{ label: string }>();</script><template><p>{{ label.toFixed() }}</p></template>',
    );
    const failed = await run([BUN, "--bun", prepare]);
    expect(failed.status).not.toBe(0);
    expect(failed.stdout + failed.stderr).toContain("TS2551");
    expect(existsSync(declaration)).toBe(false);
    const missing = await run(args);
    expect(missing.status).not.toBe(0);
    expect(ruleIds(parseReport(missing.stdout)).has("@typescript-eslint/no-unsafe-return")).toBe(
      true,
    );
  });

  test("indexed_access_preserves_missing_route_fallbacks", async () => {
    const source = `<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";
const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
</script><template><p>{{ slug }}</p></template>`;
    let [result, report] = await lint("ProofRoute.vue", source);
    expect(result.status, JSON.stringify(report)).toBe(0);
    const dictionary =
      'export function read(values: Record<string, string>): string { return values.slug ?? ""; }';
    [result, report] = await lint("ProofDictionary.ts", dictionary);
    expect(result.status, JSON.stringify(report)).toBe(0);
    const valid = await compiler("apps/web/tsconfig.eslint.json", dictionary, "ts", "tsc");
    expect(valid.status, valid.stdout + valid.stderr).toBe(0);
    const invalid = await compiler(
      "apps/web/tsconfig.eslint.json",
      dictionary.replace('values.slug ?? ""', "values.slug"),
      "ts",
      "tsc",
    );
    expect(invalid.status).not.toBe(0);
    expect(invalid.stdout + invalid.stderr).toContain("TS2322");
    expect(invalid.stdout + invalid.stderr).toContain("undefined");
    await assertRule(
      "ProofKnownField.ts",
      'export function read(value: { slug: string }): string { return value.slug ?? ""; }',
      "@typescript-eslint/no-unnecessary-condition",
    );
  });

  test("multiple_declaration_projects_fail_closed_together", async () => {
    const app = writeSource(
      "ProofApp.vue",
      `<script setup lang="ts">
defineProps<{ label: string }>();
</script><template><p>{{ label }}</p></template>`,
    );
    const editor = writeSource(
      "ProofEditor.vue",
      `<script setup lang="ts">
import type { NodeViewProps } from "@tiptap/vue-3";
defineProps<NodeViewProps>();
</script><template><span /></template>`,
    );
    const output = join(directory, "combined");
    const projects: string[] = [];
    for (const [name, component, config] of [
      ["editor", editor, "packages/editor/tsconfig.vue.json"],
      ["web", app, "apps/web/tsconfig.vue.json"],
    ] as const) {
      const project = join(directory, `tsconfig.${name}.emit.json`);
      writeFileSync(
        project,
        JSON.stringify({
          extends: join(ROOT, config),
          compilerOptions: { rootDir: directory, incremental: false, composite: false },
          include: [component],
          exclude: [],
        }),
      );
      projects.push(project);
    }
    const prepare = prepareScript("prepare-combined.mjs", projects, output);
    const emitted = await run([BUN, "--bun", prepare]);
    expect(emitted.status, emitted.stdout + emitted.stderr).toBe(0);
    for (const name of ["ProofApp.vue.d.ts", "ProofEditor.vue.d.ts"]) {
      const path = join(output, name);
      expect(existsSync(path) && statSync(path).isFile(), name).toBe(true);
    }
    const consumer = writeSource(
      "ProofCombined.ts",
      `import { createApp } from "vue";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
import ProofApp from "./ProofApp.vue";
import ProofEditor from "./ProofEditor.vue";
export const app = createApp(ProofApp);
export const renderer = VueNodeViewRenderer(ProofEditor);
export function read(value: InstanceType<typeof ProofApp>): string { return value.$props.label; }`,
    );
    const lintProject = join(directory, "tsconfig.combined.json");
    writeFileSync(
      lintProject,
      JSON.stringify({
        extends: join(ROOT, "apps/web/tsconfig.eslint.json"),
        compilerOptions: { rootDirs: [directory, output] },
        include: [consumer],
        exclude: [],
      }),
    );
    const args = [
      ...ESLINT,
      consumer,
      "--max-warnings=0",
      "--format=json",
      "--parser-options",
      JSON.stringify({ project: [lintProject] }),
    ];
    const typed = await run(args);
    expect(typed.status, typed.stdout + typed.stderr).toBe(0);
    // Reject the second project after the first has emitted fresh output.
    writeFileSync(
      app,
      '<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>',
    );
    const failed = await run([BUN, "--bun", prepare]);
    expect(failed.status).not.toBe(0);
    expect(failed.stdout + failed.stderr).toContain("TS2322");
    expect(existsSync(output)).toBe(false);
    const missing = await run(args);
    expect(missing.status).not.toBe(0);
    const reported = messages(parseReport(missing.stdout));
    expect(
      reported.filter((message) => message.ruleId === "@typescript-eslint/no-unsafe-argument")
        .length,
      JSON.stringify(reported),
    ).toBe(2);
  });

  test("node_test_runner_failure_propagation_and_no_waiver", async () => {
    for (const [name, body, expected] of [
      ["Pass", '() => { if (Number("1") !== 1) throw new Error("unexpected"); }', 0],
      ["Throw", '() => { throw new Error("node-test-throw-proof"); }', 1],
      ["Reject", '() => Promise.reject(new Error("node-test-reject-proof"))', 1],
    ] as const) {
      const path = writeSource(
        `NodeRunner${name}.test.ts`,
        `import test from "node:test"; test("${name}", ${body});`,
      );
      const runtime = await run([BUN, "test", "--isolate", path], { timeout: 30_000 });
      expect(runtime.status, `${name}: ${runtime.stdout}${runtime.stderr}`).toBe(expected);
      expect(runtime.stderr, name).toContain(expected === 0 ? "1 pass" : "1 fail");
    }
    // Installed TestContext.test has the SAME typeof test as registration;
    // a known-safe-call type allowance would also mask an unsafe subtest.
    await assertRule(
      "NodeUnhandled.test.ts",
      'import test from "node:test"; await test("parent", (t) => { t.test("child", () => Promise.resolve()); });',
      "@typescript-eslint/no-floating-promises",
    );
    const unrelated =
      'import test from "node:test"; await test("parent", () => { Promise.reject(new Error("unhandled-promise-proof")); });';
    await assertRule("NodeUnrelated.test.ts", unrelated, "@typescript-eslint/no-floating-promises");
    const runtime = await run(
      [BUN, "test", "--isolate", join(directory, "NodeUnrelated.test.ts")],
      {
        timeout: 30_000,
      },
    );
    expect(runtime.status, runtime.stdout + runtime.stderr).not.toBe(0);
    expect(runtime.stderr).toContain("unhandled-promise-proof");
  });

  test.each([
    "apps/web/src/vue/features/settings/toggle.ts",
    "apps/web/src/features/attachments/hwp-worker.ts",
    "packages/i18n/src/index.ts",
  ])("qualified_runtime_globals_and_local_names %s", async (path) => {
    const valid = await stdinLint(
      path,
      `export function local(process: { pid: number }): number { return process.pid; }
export function localObject(window: { process: { pid: number } }): number { return window.process.pid; }
export const label = { process: "local", Bun: "local" };`,
    );
    expect(valid.status, `${path}: ${valid.stdout}${valid.stderr}`).toBe(0);
    const invalid = await stdinLint(
      path,
      `export const qualified = [globalThis.process.pid,
window.process.pid, self.process.pid, globalThis.Bun.version,
window.Bun.version, self.Bun.version];`,
    );
    const report = parseReport(invalid.stdout);
    const detail = `${path}: ${JSON.stringify(report)}`;
    expect(invalid.status, detail).not.toBe(0);
    expect(
      report.reduce((sum, file) => sum + file.fatalErrorCount, 0),
      detail,
    ).toBe(0);
    const restricted = messages(report).filter(
      (message) => message.ruleId === "no-restricted-globals",
    );
    expect(restricted.length, detail).toBe(6);
    expect(
      restricted.every((message) => message.severity === 2),
      detail,
    ).toBe(true);
  });

  test("runtime_and_positive_sfc", async () => {
    const runtime = await run([
      BUN,
      "-e",
      "console.log(JSON.stringify({bun:process.versions.bun,execPath:process.execPath}))",
    ]);
    expect((JSON.parse(runtime.stdout) as { bun: string }).bun).toBe("1.4.2");
    const [result, report] = await lint("ProofValid.vue", VALID);
    expect(result.status, JSON.stringify(report)).toBe(0);
    for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]) {
      const compiled = await compiler(config, VALID);
      expect(compiled.status, `${config}: ${compiled.stdout}${compiled.stderr}`).toBe(0);
    }
    const formatted = await run(
      [...PRETTIER, "--stdin-filepath", join(directory, "ProofValid.vue")],
      {
        input: VALID,
      },
    );
    expect(formatted.status, formatted.stderr).toBe(0);
    expect(formatted.stdout).toContain("color: red;\n");
    expect(formatted.stdout).toContain(':key="row.id"');
    expect(formatted.stdout).toContain('emit("select", id);\n');
    const path = writeSource("ProofValid.vue", formatted.stdout);
    const checked = await run([...PRETTIER, "--check", path]);
    expect(checked.status, checked.stdout + checked.stderr).toBe(0);
    writeFileSync(path, VALID);
    expect((await run([...PRETTIER, "--check", path])).status).not.toBe(0);
  });

  const strictCases: [string, string][] = [
    [
      '<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>',
      "TS2322",
    ],
    [
      '<script setup lang="ts">const value = 1;</script><template><p>{{ value.toUpperCase() }}</p></template>',
      "TS2339",
    ],
    ["<template><p>{{ missingTemplateName }}</p></template>", "TS2339"],
    ["<template><UnknownComponent /></template>", "UnknownComponent"],
    [
      '<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild :label="1" /></template>',
      "TS2322",
    ],
    [
      '<script setup lang="ts">const emit = defineEmits<{ select: [id: number] }>(); emit("select", "bad");</script><template><p>bad</p></template>',
      "TS2345",
    ],
    [
      '<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild label="ok" v-slot="{ row }"><p>{{ row.missing }}</p></ProofChild></template>',
      "TS2339",
    ],
  ];
  test.each(
    ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"].flatMap((config) =>
      strictCases.map(([source, expected], index) => [config, index, expected, source] as const),
    ),
  )(
    "strict_script_template_props_emits_and_slots_types %s case %d %s",
    async (config, _index, expected, source) => {
      const result = await compiler(config, source);
      const detail = `${config} ${expected}: ${result.stdout}${result.stderr}`;
      expect(result.status, detail).not.toBe(0);
      expect(result.stdout + result.stderr, detail).toContain(expected);
    },
  );

  test.each(["apps/web/tsconfig.app.json", "packages/editor/tsconfig.json"])(
    "strict_script_template_props_emits_and_slots_types tsc %s",
    async (config) => {
      const result = await compiler(config, 'export const value: number = "bad";', "ts", "tsc");
      expect(result.status, config).not.toBe(0);
      expect(result.stdout, config).toContain("TS2322");
    },
  );

  test("unprepared_sfc_import_has_no_fake_fallback", async () => {
    writeSource(
      "ProofNode.vue",
      `<script setup lang="ts">
import type { NodeViewProps } from "@tiptap/vue-3";
defineProps<NodeViewProps>();
</script><template><span /></template>`,
    );
    const source = `import ProofNode from "./ProofNode.vue";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
export const renderer = VueNodeViewRenderer(ProofNode);`;
    // Document the plain-TS program's SFC import gap without a shim/waiver.
    await assertRule("ProofSfcImport.ts", source, "@typescript-eslint/no-unsafe-argument");
    for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]) {
      const typed = await compiler(config, source, "ts", "vue-tsc");
      expect(typed.status, `${config}: ${typed.stdout}${typed.stderr}`).toBe(0);
      const broken = await compiler(
        config,
        `${source}\nexport const invalid: number = "bad";`,
        "ts",
        "vue-tsc",
      );
      expect(broken.status, config).not.toBe(0);
      expect(broken.stdout + broken.stderr, config).toContain("TS2322");
    }
  });

  test("unused_disables_warnings_and_unmatched_paths_fail", async () => {
    let [result, report] = await lint(
      "ProofDisable.ts",
      "// eslint-disable-next-line no-debugger\nexport const value = 1;\n",
    );
    expect(result.status).not.toBe(0);
    expect(
      messages(report).some(
        (message) => message.message.includes("Unused eslint-disable") && message.severity === 2,
      ),
      JSON.stringify(report),
    ).toBe(true);
    [result, report] = await lint(
      "ProofWarning.vue",
      "<template><div v-html=\"'content'\" /></template>",
    );
    const detail = JSON.stringify(report);
    expect(
      report.reduce((sum, file) => sum + file.errorCount, 0),
      detail,
    ).toBe(0);
    expect(
      report.reduce((sum, file) => sum + file.warningCount, 0),
      detail,
    ).toBeGreaterThan(0);
    expect(result.status).not.toBe(0);
    expect((await run([...ESLINT, join(directory, "missing.ts")])).status).not.toBe(0);
    expect((await run([...PRETTIER, "--check", join(directory, "missing.ts")])).status).not.toBe(0);
  });

  test("y_text_declared_string_contract_without_rule_allowance", async () => {
    const source =
      'import * as Y from "yjs"; export function text(value: Y.Text): string { return value.toJSON(); }';
    const [result, report] = await lint("ProofYText.ts", source);
    expect(result.status, JSON.stringify(report)).toBe(0);
    await assertRule(
      "ProofYTextInherited.ts",
      source.replace("value.toJSON()", "value.toString()"),
      "@typescript-eslint/no-base-to-string",
    );
    await assertRule(
      "ProofObjectString.ts",
      "export function text(value: object): string { return value.toString(); }",
      "@typescript-eslint/no-base-to-string",
    );
  });
});
