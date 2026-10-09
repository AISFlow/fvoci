import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { createProgram } from "@typescript-eslint/parser";
import { ESLint, type Linter } from "eslint";
import * as prettier from "prettier";

export const root = resolve(import.meta.dir, "../..");
export const child =
  '<script setup lang="ts">\ndefineProps<{ label: string }>();\ndefineSlots<{ default(props: { row: { id: number; label: string } }): unknown }>();\n</script>\n<template><slot :row="{ id: 1, label }" /></template>\n';
export const valid =
  '<script setup lang="ts">\nimport UButton from "@nuxt/ui/components/Button.vue";\nimport { ref } from "vue";\nimport ProofChild from "./ProofChild.vue";\nimport type { ImportedProps } from "./props";\n\ndefineProps<ImportedProps & { enabled: boolean }>();\nconst emit = defineEmits<{ select: [id: number] }>();\ndefineSlots<{ default(props: { value: string }): unknown }>();\nconst model = ref("");\nconst rows = [{ id: 1, label: "one" }];\nfunction select(id: number): void { emit("select", id); }\n</script>\n<template>\n  <section v-if="enabled" class="flex gap-2 hover:bg-teal-50">\n    <label>Search<input v-model="model" name="search" /></label>\n    <ProofChild :label="title">\n      <template #default="{ row }"><span>{{ row.label }}</span></template>\n    </ProofChild>\n    <UButton v-for="row in rows" :key="row.id" type="button" @click="select(row.id)">{{ row.label }}</UButton>\n    <slot :value="model" />\n  </section>\n</template>\n<style scoped>\n.host :deep(.child) { color: red; }\n.host :slotted(span) { color: blue; }\n</style>\n';

// The corpus is the same as scripts/test_eslint.py. The standard ESLint API
// replaces CLI print-config/JSON capture, never the actual config or rules.
export function command(
  args: readonly string[],
  input?: string,
): { status: number | null; stdout: string; stderr: string } {
  const result = spawnSync(args[0] ?? "", args.slice(1), {
    cwd: root,
    input,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}
function success(result: ReturnType<typeof command>): void {
  assert.equal(result.status, 0, result.stdout + result.stderr);
}
function rejection(result: ReturnType<typeof command>, diagnostic?: string): void {
  assert.notEqual(result.status, 0, result.stdout + result.stderr);
  assert.notEqual(result.status, null, result.stdout + result.stderr);
  if (diagnostic !== undefined)
    assert((result.stdout + result.stderr).includes(diagnostic), result.stdout + result.stderr);
}
function lintSuccess(results: readonly ESLint.LintResult[]): void {
  assert(results.length > 0);
  assert.equal(
    results.reduce((count, result) => count + result.errorCount + result.warningCount, 0),
    0,
    JSON.stringify(results),
  );
}
function lintRejected(results: readonly ESLint.LintResult[]): void {
  assert(
    results.some((result) => result.errorCount + result.warningCount > 0),
    JSON.stringify(results),
  );
}
function messages(results: readonly ESLint.LintResult[]): Linter.LintMessage[] {
  return results.flatMap((result) => result.messages);
}
const forbidden = [
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
const runtimePaths = [
  ["apps/web/src/vue/features/settings/toggle.ts", "browser"],
  ["apps/web/src/features/attachments/hwp-worker.ts", "worker"],
  ["apps/web/src/features/attachments/pptx-worker.ts", "worker"],
  ["apps/web/src/features/attachments/xlsx-worker.ts", "worker"],
  ["apps/web/public/sw.js", "worker"],
  ["packages/i18n/src/index.ts", "library"],
] as const;

class Proof {
  readonly directory: string;
  readonly real = new ESLint({ cwd: root });
  readonly fixture: ESLint;
  constructor() {
    success(command([process.execPath, "--bun", "scripts/verify-web-tools.mjs"]));
    const parent = join(root, "target/harness-web-e2e-groups-ts-1/fixtures");
    mkdirSync(parent, { recursive: true });
    this.directory = mkdtempSync(join(parent, "eslint-"));
    this.source("props.ts", "export interface ImportedProps { title: string }\n");
    this.source("ProofChild.vue", child);
    // The same real web project is extended only to include the owned temporary
    // corpus outside apps/web. No parser, global, shim or rule is replaced.
    const project = this.source(
      "tsconfig.lint.json",
      JSON.stringify({
        extends: resolve(root, "apps/web/tsconfig.eslint.json"),
        compilerOptions: { incremental: false, composite: false },
        include: ["*.ts", "*.vue"],
        exclude: [],
      }),
    );
    this.fixture = new ESLint({
      cwd: root,
      overrideConfig: { languageOptions: { parserOptions: { project: [project] } } },
    });
  }
  eslint(project: string): ESLint {
    // Declaration consumers need a fresh snapshot after emit/cleanup. Supplying
    // a program avoids the shared watch cache without clearing other callers.
    return new ESLint({
      cwd: root,
      overrideConfig: {
        languageOptions: {
          parserOptions: { project: [project], programs: [createProgram(project, root)] },
        },
      },
    });
  }
  source(name: string, content: string): string {
    const path = join(this.directory, name);
    writeFileSync(path, content);
    return path;
  }
  async lint(name: string, content: string): Promise<ESLint.LintResult[]> {
    const path = this.source(name, content);
    return this.fixture.lintText(content, { filePath: path });
  }
  async rule(name: string, content: string, rule: string): Promise<void> {
    const results = await this.lint(name, content);
    lintRejected(results);
    assert(
      messages(results).some((message) => message.ruleId === rule),
      JSON.stringify(results),
    );
  }
  compiler(
    config: string,
    source: string,
    extension = "vue",
    compiler = "vue-tsc",
  ): ReturnType<typeof command> {
    const path = this.source("TypeProof." + extension, source);
    const project = this.source(
      "tsconfig.compile.json",
      JSON.stringify({
        extends: resolve(root, config),
        compilerOptions: { incremental: false, composite: false },
        include: [path],
        exclude: [],
      }),
    );
    return command([
      process.execPath,
      "--bun",
      resolve(root, "node_modules/.bin", compiler),
      "--noEmit",
      "-p",
      project,
    ]);
  }
  async config(path: string): Promise<Linter.Config> {
    const config: unknown = await this.real.calculateConfigForFile(resolve(root, path));
    assert(config && typeof config === "object");
    return config;
  }
  async format(source: string, filepath: string): Promise<string> {
    return prettier.format(source, {
      ...(await prettier.resolveConfig(resolve(root, filepath))),
      filepath,
    });
  }
  emit(projects: string | readonly string[], output: string): ReturnType<typeof command> {
    // A typed invocation fixture calls the existing preparation API unchanged.
    const fixture = this.source(
      "prepare.ts",
      "import { prepareVueLintTypes } from " +
        JSON.stringify(resolve(root, "scripts/prepare-vue-lint-types.mjs")) +
        "; prepareVueLintTypes(" +
        JSON.stringify(projects) +
        ", " +
        JSON.stringify(output) +
        ");\n",
    );
    return command([process.execPath, "--bun", fixture]);
  }
  cleanup(): void {
    rmSync(this.directory, { recursive: true, force: true });
  }
}
function configGlobals(config: Linter.Config): Record<string, unknown> {
  const globals: unknown = config.languageOptions?.globals ?? {};
  assert(globals !== null && typeof globals === "object");
  return globals as Record<string, unknown>;
}
function restrictedOptions(config: Linter.Config): {
  globals: string[];
  checkGlobalObject: boolean;
} {
  const rule = config.rules?.["no-restricted-globals"];
  assert(Array.isArray(rule));
  const options: unknown = rule[1];
  assert(options && typeof options === "object");
  return options as { globals: string[]; checkGlobalObject: boolean };
}
function assertRestricted(
  results: readonly ESLint.LintResult[],
  count: number,
): Linter.LintMessage[] {
  lintRejected(results);
  assert.equal(
    results.reduce((sum, result) => sum + result.fatalErrorCount, 0),
    0,
  );
  const restricted = messages(results).filter(
    (message) => message.ruleId === "no-restricted-globals",
  );
  assert.equal(restricted.length, count, JSON.stringify(results));
  assert(restricted.every((message) => message.severity === 2));
  return restricted;
}

export const toolchainCases = {
  async test_runtime_and_positive_sfc(proof: Proof): Promise<void> {
    assert.equal(process.versions.bun, "1.4.2");
    lintSuccess(await proof.lint("ProofValid.vue", valid));
    for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"])
      success(proof.compiler(config, valid));
    const filepath = join(proof.directory, "ProofValid.vue");
    const formatted = await proof.format(valid, filepath);
    for (const text of ["color: red;\n", ':key="row.id"', 'emit("select", id);\n'])
      assert(formatted.includes(text));
    const options = { ...(await prettier.resolveConfig(filepath)), filepath };
    assert(await prettier.check(formatted, options));
    assert.equal(await prettier.check(valid, options), false);
  },
  async test_dynamic_slots_have_no_unused_false_positive(proof: Proof): Promise<void> {
    const source =
      '<script setup lang="ts">\nimport ProofChild from "./ProofChild.vue";\nconst slotName = "default";\n</script>\n<template><ProofChild label="ok"><template #[slotName]="{ row }"><span>{{ row.label }}</span></template></ProofChild></template>';
    lintSuccess(await proof.lint("ProofDynamic.vue", source));
    rejection(
      proof.compiler(
        "apps/web/tsconfig.vue.json",
        source.replace("#[slotName]", "#[missingSlotName]"),
      ),
      "missingSlotName",
    );
  },
  async test_directives_unused_any_and_typed_promises_fail(proof: Proof): Promise<void> {
    const cases = [
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
    ] as const;
    for (const [name, source, rule] of cases) await proof.rule(name, source, rule);
  },
  test_strict_script_template_props_emits_and_slots_types(proof: Proof): void {
    const cases = [
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
    ] as const;
    for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"])
      for (const [source, expected] of cases) rejection(proof.compiler(config, source), expected);
    for (const config of ["apps/web/tsconfig.app.json", "packages/editor/tsconfig.json"])
      rejection(
        proof.compiler(config, 'export const value: number = "bad";', "ts", "tsc"),
        "TS2322",
      );
  },
  test_actual_nuxt_autoimport_registration_boundary(proof: Proof): void {
    rejection(
      proof.compiler(
        "apps/web/tsconfig.vue.json",
        '<template><UButton type="button">ok</UButton></template>',
      ),
      "UButton",
    );
  },
  async test_unused_disables_warnings_and_unmatched_paths_fail(proof: Proof): Promise<void> {
    const disabled = await proof.lint(
      "ProofDisable.ts",
      "// eslint-disable-next-line no-debugger\nexport const value = 1;\n",
    );
    lintRejected(disabled);
    assert(
      messages(disabled).some(
        (message) => message.message.includes("Unused eslint-disable") && message.severity === 2,
      ),
    );
    const warnings = await proof.lint(
      "ProofWarning.vue",
      "<template><div v-html=\"'content'\" /></template>",
    );
    assert.equal(
      warnings.reduce((sum, result) => sum + result.errorCount, 0),
      0,
    );
    assert(warnings.some((result) => result.warningCount > 0));
    lintRejected(warnings);
    await assert.rejects(proof.fixture.lintFiles(join(proof.directory, "missing.ts")));
    rejection(
      command([
        process.execPath,
        "--bun",
        resolve(root, "node_modules/prettier/bin/prettier.cjs"),
        "--check",
        join(proof.directory, "missing.ts"),
      ]),
    );
  },
  async test_browser_does_not_receive_node_or_bun_globals(proof: Proof): Promise<void> {
    for (const [path, environment] of runtimePaths) {
      const config = await proof.config(path);
      const declared = configGlobals(config);
      assert(!forbidden.some((name) => name in declared));
      const rule = restrictedOptions(config);
      assert.deepEqual(
        new Set(rule.globals),
        new Set(environment === "worker" ? [...forbidden, "window"] : forbidden),
      );
      assert(rule.checkGlobalObject);
      if (environment === "browser") assert("window" in declared);
      if (environment === "worker") {
        assert("self" in declared);
        assert("postMessage" in declared);
        assert(!("window" in declared));
      }
      if (path.endsWith(".ts")) {
        const promises = config.rules?.["@typescript-eslint/no-floating-promises"];
        assert(Array.isArray(promises));
        assert.equal((promises[1] as { ignoreVoid: boolean }).ignoreVoid, false);
      }
    }
  },
  async test_browser_worker_and_i18n_reject_runtime_node_bun(proof: Proof): Promise<void> {
    const negative =
      'export const forbidden = [process.pid, Bun.version,\nBuffer.alloc(0), require("bad"), __dirname, __filename, module, exports,\nsetImmediate, clearImmediate];';
    for (const [path, environment] of runtimePaths) {
      const positive = {
        browser: "export const href = window.location.href;",
        worker: 'self.postMessage("ready");',
        library: "export const add = (left: number, right: number): number => left + right;",
      }[environment];
      lintSuccess(await proof.real.lintText(positive, { filePath: resolve(root, path) }));
      const restricted = assertRestricted(
        await proof.real.lintText(negative, { filePath: resolve(root, path) }),
        10,
      );
      assert.deepEqual(
        new Set(restricted.map((message) => message.message.split("'")[1])),
        new Set(forbidden),
      );
    }
  },
  async test_qualified_runtime_globals_and_local_names(proof: Proof): Promise<void> {
    const positive =
      'export function local(process: { pid: number }): number { return process.pid; }\nexport function localObject(window: { process: { pid: number } }): number { return window.process.pid; }\nexport const label = { process: "local", Bun: "local" };';
    const negative =
      "export const qualified = [globalThis.process.pid,\nwindow.process.pid, self.process.pid, globalThis.Bun.version,\nwindow.Bun.version, self.Bun.version];";
    for (const path of [
      "apps/web/src/vue/features/settings/toggle.ts",
      "apps/web/src/features/attachments/hwp-worker.ts",
      "packages/i18n/src/index.ts",
    ]) {
      lintSuccess(await proof.real.lintText(positive, { filePath: resolve(root, path) }));
      assertRestricted(await proof.real.lintText(negative, { filePath: resolve(root, path) }), 6);
    }
  },
  async test_indexed_access_preserves_missing_route_fallbacks(proof: Proof): Promise<void> {
    const source =
      '<script setup lang="ts">\nimport { computed } from "vue";\nimport { useRoute } from "vue-router";\nconst route = useRoute();\nconst slug = computed(() => String(route.params.slug ?? ""));\n</script><template><p>{{ slug }}</p></template>';
    lintSuccess(await proof.lint("ProofRoute.vue", source));
    const dictionary =
      'export function read(values: Record<string, string>): string { return values.slug ?? ""; }';
    lintSuccess(await proof.lint("ProofDictionary.ts", dictionary));
    success(proof.compiler("apps/web/tsconfig.eslint.json", dictionary, "ts", "tsc"));
    const invalid = proof.compiler(
      "apps/web/tsconfig.eslint.json",
      dictionary.replace('values.slug ?? ""', "values.slug"),
      "ts",
      "tsc",
    );
    rejection(invalid, "TS2322");
    assert((invalid.stdout + invalid.stderr).includes("undefined"));
    await proof.rule(
      "ProofKnownField.ts",
      'export function read(value: { slug: string }): string { return value.slug ?? ""; }',
      "@typescript-eslint/no-unnecessary-condition",
    );
  },
  async test_exact_development_export_buffer_contract(proof: Proof): Promise<void> {
    for (const path of [
      "packages/editor/src/export/docx.ts",
      "packages/editor/src/export/pptx.ts",
    ]) {
      const filePath = resolve(root, path);
      lintSuccess(
        await proof.real.lintText('export const bytes = Buffer.from("oracle", "utf8");', {
          filePath,
        }),
      );
      const config = await proof.config(path);
      assert.equal(configGlobals(config)["Buffer"], "readonly");
      assert.deepEqual(
        new Set(restrictedOptions(config).globals),
        new Set(forbidden.filter((name) => name !== "Buffer")),
      );
      assertRestricted(
        await proof.real.lintText(
          "export const forbidden = [process.pid, Bun.version, globalThis.process.pid, globalThis.Bun.version];",
          { filePath },
        ),
        4,
      );
    }
    for (const path of [
      "packages/editor/src/json.ts",
      "packages/editor/src/export/limits.ts",
      "apps/web/src/features/attachments/hwp-worker.ts",
      "packages/i18n/src/index.ts",
    ]) {
      const results = await proof.real.lintText("export const bytes = Buffer.alloc(0);", {
        filePath: resolve(root, path),
      });
      lintRejected(results);
      assert(messages(results).some((message) => message.ruleId === "no-restricted-globals"));
    }
  },
  async test_bun_development_types_and_runtime(proof: Proof): Promise<void> {
    const source =
      'import assert from "node:assert/strict";\nimport { mock, test } from "bun:test";\nawait mock.module("fvoci-bun-typing-proof", () => ({ value: 1 }));\ntest("typed Bun Transpiler and module mock", () => {\n  const code = new Bun.Transpiler({ loader: "ts" }).transformSync("export const value: number = 1;");\n  assert.equal(typeof code, "string");\n  assert.equal(code.includes("number"), false);\n});';
    lintSuccess(await proof.lint("ProofBun.test.ts", source));
    const runtime = command([
      process.execPath,
      "test",
      "--isolate",
      join(proof.directory, "ProofBun.test.ts"),
    ]);
    success(runtime);
    assert(runtime.stderr.includes("1 pass"));
    for (const config of [
      "apps/web/tsconfig.eslint.json",
      "packages/editor/tsconfig.eslint.json",
    ]) {
      success(proof.compiler(config, source, "ts", "tsc"));
      rejection(
        proof.compiler(
          config,
          source.replace('loader: "ts"', 'loader: "invalid-loader"'),
          "ts",
          "tsc",
        ),
        "TS2322",
      );
    }
    await proof.rule(
      "ProofBunUnsafe.test.ts",
      'import { mock } from "bun:test"; mock.module("proof", () => ({ value: 1 }));',
      "@typescript-eslint/no-floating-promises",
    );
    await proof.rule(
      "ProofBunUnrelated.test.ts",
      'import { test } from "bun:test"; test("proof", () => { Promise.resolve(1); });',
      "@typescript-eslint/no-floating-promises",
    );
  },
  async test_node_test_runner_failure_propagation_and_no_waiver(proof: Proof): Promise<void> {
    for (const [name, body, expected] of [
      ["Pass", '() => { if (Number("1") !== 1) throw new Error("unexpected"); }', 0],
      ["Throw", '() => { throw new Error("node-test-throw-proof"); }', 1],
      ["Reject", '() => Promise.reject(new Error("node-test-reject-proof"))', 1],
    ] as const) {
      const path = proof.source(
        "NodeRunner" + name + ".test.ts",
        'import test from "node:test"; test(' + JSON.stringify(name) + ", " + body + ");",
      );
      const runtime = command([process.execPath, "test", "--isolate", path]);
      assert.equal(runtime.status, expected, runtime.stdout + runtime.stderr);
      assert(runtime.stderr.includes(expected === 0 ? "1 pass" : "1 fail"));
    }
    await proof.rule(
      "NodeUnhandled.test.ts",
      'import test from "node:test"; await test("parent", (t) => { t.test("child", () => Promise.resolve()); });',
      "@typescript-eslint/no-floating-promises",
    );
    await proof.rule(
      "NodeUnrelated.test.ts",
      'import test from "node:test"; await test("parent", () => { Promise.reject(new Error("unhandled-promise-proof")); });',
      "@typescript-eslint/no-floating-promises",
    );
    const runtime = command([
      process.execPath,
      "test",
      "--isolate",
      join(proof.directory, "NodeUnrelated.test.ts"),
    ]);
    rejection(runtime, "unhandled-promise-proof");
  },
  async test_actual_web_and_editor_declaration_preparation_is_node_free(
    proof: Proof,
  ): Promise<void> {
    success(command([process.execPath, "--bun", "scripts/prepare-vue-lint-types.mjs"]));
    const output = resolve(root, "node_modules/.cache/fvoci-vue-lint/types");
    assert(
      readFileSync(join(output, "apps/web/src/vue/App.vue.d.ts"), "utf8").includes(
        'import("vue").DefineComponent',
      ),
    );
    assert.equal(
      readdirSync(join(output, "packages/editor/src/vue")).filter((file) =>
        file.endsWith(".vue.d.ts"),
      ).length,
      8,
    );
    lintSuccess(await proof.real.lintFiles("packages/editor/src/vue/node-views.ts"));
    lintSuccess(
      await proof.real.lintText(
        'import { createApp } from "vue";\nimport App from "./App.vue";\nexport const app = createApp(App);',
        { filePath: resolve(root, "apps/web/src/vue/main.ts") },
      ),
    );
  },
  async test_multiple_declaration_projects_fail_closed_together(proof: Proof): Promise<void> {
    const app = proof.source(
      "ProofApp.vue",
      '<script setup lang="ts">\ndefineProps<{ label: string }>();\n</script><template><p>{{ label }}</p></template>',
    );
    const editor = proof.source(
      "ProofEditor.vue",
      '<script setup lang="ts">\nimport type { NodeViewProps } from "@tiptap/vue-3";\ndefineProps<NodeViewProps>();\n</script><template><span /></template>',
    );
    const output = join(proof.directory, "combined");
    const projects = [
      ["editor", editor, "packages/editor/tsconfig.vue.json"],
      ["web", app, "apps/web/tsconfig.vue.json"],
    ].map(([name, component, config]) => {
      assert(name && component && config);
      return proof.source(
        "tsconfig." + name + ".emit.json",
        JSON.stringify({
          extends: resolve(root, config),
          compilerOptions: { rootDir: proof.directory, incremental: false, composite: false },
          include: [component],
          exclude: [],
        }),
      );
    });
    success(proof.emit(projects, output));
    for (const name of ["ProofApp.vue.d.ts", "ProofEditor.vue.d.ts"])
      assert(existsSync(join(output, name)));
    const consumer = proof.source(
      "ProofCombined.ts",
      'import { createApp } from "vue";\nimport { VueNodeViewRenderer } from "@tiptap/vue-3";\nimport ProofApp from "./ProofApp.vue";\nimport ProofEditor from "./ProofEditor.vue";\nexport const app = createApp(ProofApp);\nexport const renderer = VueNodeViewRenderer(ProofEditor);\nexport function read(value: InstanceType<typeof ProofApp>): string { return value.$props.label; }',
    );
    const project = proof.source(
      "tsconfig.combined.json",
      JSON.stringify({
        extends: resolve(root, "apps/web/tsconfig.eslint.json"),
        compilerOptions: { rootDirs: [proof.directory, output] },
        include: [consumer],
        exclude: [],
      }),
    );
    lintSuccess(await proof.eslint(project).lintFiles(consumer));
    writeFileSync(
      app,
      '<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>',
    );
    rejection(proof.emit(projects, output), "TS2322");
    assert(!existsSync(output));
    const missing = await proof.eslint(project).lintFiles(consumer);
    lintRejected(missing);
    assert.equal(
      messages(missing).filter(
        (message) => message.ruleId === "@typescript-eslint/no-unsafe-argument",
      ).length,
      2,
    );
  },
  async test_generated_sfc_types_and_failed_refresh_have_no_waiver(proof: Proof): Promise<void> {
    const component = proof.source(
      "ProofGenerated.vue",
      '<script setup lang="ts">\ndefineProps<{ label: string }>();\n</script><template><p>{{ label }}</p></template>',
    );
    const output = join(proof.directory, "generated");
    const project = proof.source(
      "tsconfig.emit.json",
      JSON.stringify({
        extends: resolve(root, "apps/web/tsconfig.vue.json"),
        compilerOptions: { rootDir: proof.directory, incremental: false, composite: false },
        include: [component],
        exclude: [],
      }),
    );
    success(proof.emit(project, output));
    const declaration = join(output, "ProofGenerated.vue.d.ts");
    assert(existsSync(declaration));
    const source =
      'import ProofGenerated from "./ProofGenerated.vue";\nexport function read(value: InstanceType<typeof ProofGenerated>): string { return value.$props.label; }';
    const consumer = proof.source("ProofGeneratedConsumer.ts", source);
    const lintProject = proof.source(
      "tsconfig.generated.json",
      JSON.stringify({
        extends: resolve(root, "apps/web/tsconfig.eslint.json"),
        compilerOptions: { rootDirs: [proof.directory, output] },
        include: [consumer],
        exclude: [],
      }),
    );
    lintSuccess(await proof.eslint(lintProject).lintFiles(consumer));
    writeFileSync(consumer, source.replace("value.$props.label", "value.$props.label.missing"));
    rejection(
      command([
        process.execPath,
        "--bun",
        resolve(root, "node_modules/.bin/tsc"),
        "--noEmit",
        "-p",
        lintProject,
      ]),
      "TS2339",
    );
    writeFileSync(consumer, source);
    writeFileSync(
      component,
      readFileSync(component, "utf8").replace("label: string", "label: any"),
    );
    success(proof.emit(project, output));
    const unsafe = await proof.eslint(lintProject).lintFiles(consumer);
    lintRejected(unsafe);
    assert(
      messages(unsafe).some((message) => message.ruleId === "@typescript-eslint/no-unsafe-return"),
    );
    writeFileSync(
      component,
      '<script setup lang="ts">defineProps<{ label: string }>();</script><template><p>{{ label.toFixed() }}</p></template>',
    );
    rejection(proof.emit(project, output), "TS2551");
    assert(!existsSync(declaration));
    const missing = await proof.eslint(lintProject).lintFiles(consumer);
    lintRejected(missing);
    assert(
      messages(missing).some((message) => message.ruleId === "@typescript-eslint/no-unsafe-return"),
    );
  },
  async test_unprepared_sfc_import_has_no_fake_fallback(proof: Proof): Promise<void> {
    proof.source(
      "ProofNode.vue",
      '<script setup lang="ts">\nimport type { NodeViewProps } from "@tiptap/vue-3";\ndefineProps<NodeViewProps>();\n</script><template><span /></template>',
    );
    const source =
      'import ProofNode from "./ProofNode.vue";\nimport { VueNodeViewRenderer } from "@tiptap/vue-3";\nexport const renderer = VueNodeViewRenderer(ProofNode);';
    await proof.rule("ProofSfcImport.ts", source, "@typescript-eslint/no-unsafe-argument");
    for (const config of ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]) {
      success(proof.compiler(config, source, "ts", "vue-tsc"));
      rejection(
        proof.compiler(config, source + '\nexport const invalid: number = "bad";', "ts", "vue-tsc"),
        "TS2322",
      );
    }
  },
  async test_y_text_declared_string_contract_without_rule_allowance(proof: Proof): Promise<void> {
    const source =
      'import * as Y from "yjs"; export function text(value: Y.Text): string { return value.toJSON(); }';
    lintSuccess(await proof.lint("ProofYText.ts", source));
    await proof.rule(
      "ProofYTextInherited.ts",
      source.replace("value.toJSON()", "value.toString()"),
      "@typescript-eslint/no-base-to-string",
    );
    await proof.rule(
      "ProofObjectString.ts",
      "export function text(value: object): string { return value.toString(); }",
      "@typescript-eslint/no-base-to-string",
    );
  },
  async test_formatter_keeps_import_order_text_and_tailwind(proof: Proof): Promise<void> {
    const ordered = await proof.format(
      'import "./z.css";\nimport "./a.css";\nexport const value = 1;\n',
      "order.ts",
    );
    assert(ordered.indexOf('"./z.css"') < ordered.indexOf('"./a.css"'));
    const text = await proof.format(
      "<template><p>before <strong>middle</strong> after</p><pre>  keep\n spacing </pre></template>",
      "text.vue",
    );
    assert(text.includes("before <strong>middle</strong> after"));
    assert(text.includes("  keep\n spacing "));
    await proof.format(
      '@import "tailwindcss" source(none);\n@source "./";\n@theme { --color-brand: #123456; }\n@utility proof { @apply flex; }\n',
      "proof.css",
    );
  },
};

export type ToolchainCaseName = keyof typeof toolchainCases;
export async function runToolchainCase(name: ToolchainCaseName): Promise<void> {
  const proof = new Proof();
  try {
    await toolchainCases[name](proof);
  } finally {
    proof.cleanup();
  }
}
