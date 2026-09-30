#!/usr/bin/env python3
"""Positive and negative proofs for the pinned Bun-only Vue toolchain."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
ESLINT = ["bun", "--bun", str(ROOT / "node_modules/eslint/bin/eslint.js")]
PRETTIER = ["bun", "--bun", str(ROOT / "node_modules/prettier/bin/prettier.cjs")]


class VueToolchain(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        verified = run(["bun", "--bun", "scripts/verify-web-tools.mjs"])
        if verified.returncode:
            raise AssertionError(verified.stdout + verified.stderr)
        cls.temp = tempfile.TemporaryDirectory(prefix="eslint-proof-", dir=ROOT / "apps/web")
        cls.directory = Path(cls.temp.name)
        (cls.directory / "props.ts").write_text("export interface ImportedProps { title: string }\n")
        (cls.directory / "ProofChild.vue").write_text(CHILD)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def source(self, name, source):
        path = self.directory / name
        path.write_text(source)
        return path

    def lint(self, name, source, *options):
        path = self.source(name, source)
        result = run([*ESLINT, str(path), "--max-warnings=0", "--format=json", *options])
        return result, json.loads(result.stdout)

    def assert_rule(self, name, source, rule):
        result, report = self.lint(name, source)
        self.assertNotEqual(result.returncode, 0, report)
        self.assertIn(rule, {m["ruleId"] for f in report for m in f["messages"]}, report)

    def compiler(self, config, source, extension="vue", compiler="vue-tsc"):
        path = self.source(f"TypeProof.{extension}", source)
        config_path = self.directory / "tsconfig.json"
        config_path.write_text(json.dumps({
            "extends": str(ROOT / config),
            "compilerOptions": {"incremental": False, "composite": False},
            "include": [str(path)], "exclude": [],
        }))
        return run(["bun", "--bun", str(ROOT / "node_modules/.bin" / compiler),
                    "--noEmit", "-p", str(config_path)])

    def test_runtime_and_positive_sfc(self):
        runtime = run(["bun", "-e", 'console.log(JSON.stringify({bun:process.versions.bun,execPath:process.execPath}))'])
        self.assertEqual(json.loads(runtime.stdout)["bun"], "1.4.2")
        result, report = self.lint("ProofValid.vue", VALID)
        self.assertEqual(result.returncode, 0, report)
        for config in ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]:
            result = self.compiler(config, VALID)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        formatted = run([*PRETTIER, "--stdin-filepath", str(self.directory / "ProofValid.vue")], input=VALID)
        self.assertEqual(formatted.returncode, 0, formatted.stderr)
        self.assertIn("color: red;\n", formatted.stdout)
        self.assertIn(':key="row.id"', formatted.stdout)
        self.assertIn('emit("select", id);\n', formatted.stdout)
        path = self.source("ProofValid.vue", formatted.stdout)
        checked = run([*PRETTIER, "--check", str(path)])
        self.assertEqual(checked.returncode, 0, checked.stdout + checked.stderr)
        path.write_text(VALID)
        self.assertNotEqual(run([*PRETTIER, "--check", str(path)]).returncode, 0)

    def test_dynamic_slots_have_no_unused_false_positive(self):
        source = '''<script setup lang="ts">
import ProofChild from "./ProofChild.vue";
const slotName = "default";
</script>
<template><ProofChild label="ok"><template #[slotName]="{ row }"><span>{{ row.label }}</span></template></ProofChild></template>'''
        result, report = self.lint("ProofDynamic.vue", source)
        self.assertEqual(result.returncode, 0, report)
        result = self.compiler("apps/web/tsconfig.vue.json", source.replace("#[slotName]", "#[missingSlotName]"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missingSlotName", result.stdout)

    def test_directives_unused_any_and_typed_promises_fail(self):
        cases = [
            ("ProofUnused.vue", '<script setup lang="ts">const unused = 1;</script><template><p>ok</p></template>', "@typescript-eslint/no-unused-vars"),
            ("ProofUnusedImport.vue", '<script setup lang="ts">import { ref } from "vue";</script><template><p>ok</p></template>', "@typescript-eslint/no-unused-vars"),
            ("ProofParse.vue", '<template><p v-if="(">bad</p></template>', "vue/no-parsing-error"),
            ("ProofFor.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows">{{ row }}</p></template>', "vue/require-v-for-key"),
            ("ProofKey.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows" :key="1">{{ row }}</p></template>', "vue/valid-v-for"),
            ("ProofIfFor.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-if="true" v-for="row in rows" :key="row">{{ row }}</p></template>', "vue/no-use-v-if-with-v-for"),
            ("ProofIf.vue", '<template><p v-if>bad</p></template>', "vue/valid-v-if"),
            ("ProofModel.vue", '<template><input v-model="1" /></template>', "vue/valid-v-model"),
            ("ProofAny.ts", 'export const value: any = 1;', "@typescript-eslint/no-explicit-any"),
            ("ProofUnsafe.ts", 'export function read(value: any): string { return value; }', "@typescript-eslint/no-unsafe-return"),
            ("ProofPromise.ts", 'export const save = (): Promise<number> => Promise.resolve(1); save();', "@typescript-eslint/no-floating-promises"),
            ("ProofVoid.ts", 'export const save = (): Promise<number> => Promise.resolve(1); void save();', "@typescript-eslint/no-floating-promises"),
        ]
        for name, source, rule in cases:
            with self.subTest(name=name):
                self.assert_rule(name, source, rule)

    def test_strict_script_template_props_emits_and_slots_types(self):
        cases = [
            ('<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>', "TS2322"),
            ('<script setup lang="ts">const value = 1;</script><template><p>{{ value.toUpperCase() }}</p></template>', "TS2339"),
            ('<template><p>{{ missingTemplateName }}</p></template>', "TS2339"),
            ('<template><UnknownComponent /></template>', "UnknownComponent"),
            ('<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild :label="1" /></template>', "TS2322"),
            ('<script setup lang="ts">const emit = defineEmits<{ select: [id: number] }>(); emit("select", "bad");</script><template><p>bad</p></template>', "TS2345"),
            ('<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild label="ok" v-slot="{ row }"><p>{{ row.missing }}</p></ProofChild></template>', "TS2339"),
        ]
        for config in ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]:
            for source, expected in cases:
                with self.subTest(config=config, expected=expected):
                    result = self.compiler(config, source)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn(expected, result.stdout + result.stderr)
        for config in ["apps/web/tsconfig.app.json", "packages/editor/tsconfig.json"]:
            result = self.compiler(config, 'export const value: number = "bad";', "ts", "tsc")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("TS2322", result.stdout)

    def test_actual_nuxt_autoimport_registration_boundary(self):
        # FVOCI disables Nuxt UI autoimports: an unregistered template name must fail.
        result = self.compiler("apps/web/tsconfig.vue.json", '<template><UButton type="button">ok</UButton></template>')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("UButton", result.stdout)

    def test_unused_disables_warnings_and_unmatched_paths_fail(self):
        result, report = self.lint("ProofDisable.ts", '// eslint-disable-next-line no-debugger\nexport const value = 1;\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any("Unused eslint-disable" in m["message"] and m["severity"] == 2 for f in report for m in f["messages"]))
        result, report = self.lint("ProofWarning.vue", '<template><div v-html="\'content\'" /></template>')
        self.assertEqual(sum(f["errorCount"] for f in report), 0, report)
        self.assertGreater(sum(f["warningCount"] for f in report), 0, report)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotEqual(run([*ESLINT, str(self.directory / "missing.ts")]).returncode, 0)
        self.assertNotEqual(run([*PRETTIER, "--check", str(self.directory / "missing.ts")]).returncode, 0)

    def test_browser_does_not_receive_node_or_bun_globals(self):
        forbidden = {"Bun", "process", "Buffer", "require", "__dirname", "__filename",
                     "module", "exports", "setImmediate", "clearImmediate"}
        for path, environment in self.runtime_paths():
            with self.subTest(path=path):
                result = run([*ESLINT, "--print-config", path])
                self.assertEqual(result.returncode, 0, result.stderr)
                config = json.loads(result.stdout)
                declared = config["languageOptions"].get("globals", {})
                self.assertFalse(forbidden.intersection(declared), declared)
                runtime_rule = config["rules"]["no-restricted-globals"][1]
                expected = forbidden | {"window"} if environment == "worker" else forbidden
                self.assertEqual(set(runtime_rule["globals"]), expected)
                self.assertTrue(runtime_rule["checkGlobalObject"])
                if environment == "browser":
                    self.assertIn("window", declared)
                elif environment == "worker":
                    self.assertIn("self", declared)
                    self.assertIn("postMessage", declared)
                    self.assertNotIn("window", declared)
                if path.endswith(".ts"):
                    self.assertEqual(config["rules"]["@typescript-eslint/no-floating-promises"][1]["ignoreVoid"], False)

    @staticmethod
    def runtime_paths():
        return [
            ("apps/web/src/vue/features/settings/toggle.ts", "browser"),
            ("apps/web/src/features/attachments/hwp-worker.ts", "worker"),
            ("apps/web/src/features/attachments/pptx-worker.ts", "worker"),
            ("apps/web/src/features/attachments/xlsx-worker.ts", "worker"),
            ("apps/web/public/sw.js", "worker"),
            ("packages/i18n/src/index.ts", "library"),
        ]

    def test_browser_worker_and_i18n_reject_runtime_node_bun(self):
        # Stdin with actual file paths exercises the real root config/projects
        # without writing product files or replacing the parser/rule set.
        negative = '''export const forbidden = [process.pid, Bun.version,
Buffer.alloc(0), require("bad"), __dirname, __filename, module, exports,
setImmediate, clearImmediate];'''
        names = {"Bun", "process", "Buffer", "require", "__dirname", "__filename",
                 "module", "exports", "setImmediate", "clearImmediate"}
        for path, environment in self.runtime_paths():
            with self.subTest(path=path):
                positive = {
                    "browser": "export const href = window.location.href;",
                    "worker": 'self.postMessage("ready");',
                    "library": "export const add = (left: number, right: number): number => left + right;",
                }[environment]
                args = [*ESLINT, "--stdin", "--stdin-filename", path,
                        "--max-warnings=0", "--format=json"]
                valid = run(args, input=positive)
                self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
                invalid = run(args, input=negative)
                report = json.loads(invalid.stdout)
                self.assertNotEqual(invalid.returncode, 0, report)
                self.assertEqual(sum(f["fatalErrorCount"] for f in report), 0, report)
                restricted = [m for f in report for m in f["messages"]
                              if m["ruleId"] == "no-restricted-globals"]
                self.assertEqual({m["message"].split("'")[1] for m in restricted}, names, report)
                self.assertTrue(all(m["severity"] == 2 for m in restricted))

    def test_qualified_runtime_globals_and_local_names(self):
        for path in ["apps/web/src/vue/features/settings/toggle.ts",
                     "apps/web/src/features/attachments/hwp-worker.ts",
                     "packages/i18n/src/index.ts"]:
            with self.subTest(path=path):
                args = [*ESLINT, "--stdin", "--stdin-filename", path,
                        "--max-warnings=0", "--format=json"]
                positive = '''export function local(process: { pid: number }): number { return process.pid; }
export function localObject(window: { process: { pid: number } }): number { return window.process.pid; }
export const label = { process: "local", Bun: "local" };'''
                valid = run(args, input=positive)
                self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
                negative = '''export const qualified = [globalThis.process.pid,
window.process.pid, self.process.pid, globalThis.Bun.version,
window.Bun.version, self.Bun.version];'''
                invalid = run(args, input=negative)
                report = json.loads(invalid.stdout)
                self.assertNotEqual(invalid.returncode, 0, report)
                self.assertEqual(sum(f["fatalErrorCount"] for f in report), 0, report)
                restricted = [m for f in report for m in f["messages"]
                              if m["ruleId"] == "no-restricted-globals"]
                self.assertEqual(len(restricted), 6, report)
                self.assertTrue(all(m["severity"] == 2 for m in restricted))

    def test_indexed_access_preserves_missing_route_fallbacks(self):
        source = '''<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";
const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
</script><template><p>{{ slug }}</p></template>'''
        result, report = self.lint("ProofRoute.vue", source)
        self.assertEqual(result.returncode, 0, report)
        dictionary = 'export function read(values: Record<string, string>): string { return values.slug ?? ""; }'
        result, report = self.lint("ProofDictionary.ts", dictionary)
        self.assertEqual(result.returncode, 0, report)
        valid = self.compiler("apps/web/tsconfig.eslint.json", dictionary, "ts", "tsc")
        self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
        invalid = self.compiler("apps/web/tsconfig.eslint.json", dictionary.replace('values.slug ?? ""', 'values.slug'), "ts", "tsc")
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn("TS2322", invalid.stdout + invalid.stderr)
        self.assertIn("undefined", invalid.stdout + invalid.stderr)
        self.assert_rule("ProofKnownField.ts", 'export function read(value: { slug: string }): string { return value.slug ?? ""; }', "@typescript-eslint/no-unnecessary-condition")

    def test_exact_development_export_buffer_contract(self):
        for path in ["packages/editor/src/export/docx.ts", "packages/editor/src/export/pptx.ts"]:
            with self.subTest(path=path):
                args = [*ESLINT, "--stdin", "--stdin-filename", path,
                        "--max-warnings=0", "--format=json"]
                positive = 'export const bytes = Buffer.from("oracle", "utf8");'
                valid = run(args, input=positive)
                self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
                config = json.loads(run([*ESLINT, "--print-config", path]).stdout)
                self.assertEqual(config["languageOptions"]["globals"]["Buffer"], "readonly")
                names = {"Bun", "process", "require", "__dirname", "__filename",
                         "module", "exports", "setImmediate", "clearImmediate"}
                self.assertEqual(set(config["rules"]["no-restricted-globals"][1]["globals"]), names)
                negative = 'export const forbidden = [process.pid, Bun.version, globalThis.process.pid, globalThis.Bun.version];'
                invalid = run(args, input=negative)
                report = json.loads(invalid.stdout)
                self.assertNotEqual(invalid.returncode, 0, report)
                self.assertEqual(sum(f["fatalErrorCount"] for f in report), 0, report)
                restricted = [m for f in report for m in f["messages"]
                              if m["ruleId"] == "no-restricted-globals"]
                self.assertEqual(len(restricted), 4, report)
        for path in ["packages/editor/src/json.ts", "packages/editor/src/export/limits.ts",
                     "apps/web/src/features/attachments/hwp-worker.ts", "packages/i18n/src/index.ts"]:
            with self.subTest(browser_path=path):
                result = run([*ESLINT, "--stdin", "--stdin-filename", path,
                              "--max-warnings=0", "--format=json"], input='export const bytes = Buffer.alloc(0);')
                report = json.loads(result.stdout)
                self.assertNotEqual(result.returncode, 0, report)
                self.assertIn("no-restricted-globals", {m["ruleId"] for f in report for m in f["messages"]}, report)

    def test_bun_development_types_and_runtime(self):
        source = '''import assert from "node:assert/strict";
import { mock, test } from "bun:test";
await mock.module("fvoci-bun-typing-proof", () => ({ value: 1 }));
test("typed Bun Transpiler and module mock", () => {
  const code = new Bun.Transpiler({ loader: "ts" }).transformSync("export const value: number = 1;");
  assert.equal(typeof code, "string");
  assert.equal(code.includes("number"), false);
});'''
        result, report = self.lint("ProofBun.test.ts", source)
        self.assertEqual(result.returncode, 0, report)
        path = self.directory / "ProofBun.test.ts"
        runtime = run(["bun", "test", "--isolate", str(path)], timeout=30)
        self.assertEqual(runtime.returncode, 0, runtime.stdout + runtime.stderr)
        self.assertIn("1 pass", runtime.stderr)
        for config in ["apps/web/tsconfig.eslint.json", "packages/editor/tsconfig.eslint.json"]:
            valid = self.compiler(config, source, "ts", "tsc")
            self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
            invalid = self.compiler(config, source.replace('loader: "ts"', 'loader: "invalid-loader"'), "ts", "tsc")
            self.assertNotEqual(invalid.returncode, 0)
            self.assertIn("TS2322", invalid.stdout + invalid.stderr)
        self.assert_rule("ProofBunUnsafe.test.ts", 'import { mock } from "bun:test"; mock.module("proof", () => ({ value: 1 }));', "@typescript-eslint/no-floating-promises")
        self.assert_rule("ProofBunUnrelated.test.ts", 'import { test } from "bun:test"; test("proof", () => { Promise.resolve(1); });', "@typescript-eslint/no-floating-promises")

    def test_node_test_runner_failure_propagation_and_no_waiver(self):
        for name, body, expected in [
            ("Pass", '() => { if (Number("1") !== 1) throw new Error("unexpected"); }', 0),
            ("Throw", '() => { throw new Error("node-test-throw-proof"); }', 1),
            ("Reject", '() => Promise.reject(new Error("node-test-reject-proof"))', 1),
        ]:
            source = f'import test from "node:test"; test("{name}", {body});'
            path = self.source(f"NodeRunner{name}.test.ts", source)
            runtime = run(["bun", "test", "--isolate", str(path)], timeout=30)
            self.assertEqual(runtime.returncode, expected, runtime.stdout + runtime.stderr)
            self.assertIn("1 pass" if expected == 0 else "1 fail", runtime.stderr)
        # Installed TestContext.test has the SAME typeof test as registration;
        # a known-safe-call type allowance would also mask an unsafe subtest.
        self.assert_rule("NodeUnhandled.test.ts", 'import test from "node:test"; await test("parent", (t) => { t.test("child", () => Promise.resolve()); });', "@typescript-eslint/no-floating-promises")
        unrelated = 'import test from "node:test"; await test("parent", () => { Promise.reject(new Error("unhandled-promise-proof")); });'
        self.assert_rule("NodeUnrelated.test.ts", unrelated, "@typescript-eslint/no-floating-promises")
        path = self.directory / "NodeUnrelated.test.ts"
        runtime = run(["bun", "test", "--isolate", str(path)], timeout=30)
        self.assertNotEqual(runtime.returncode, 0, runtime.stdout + runtime.stderr)
        self.assertIn("unhandled-promise-proof", runtime.stderr)

    def test_actual_web_and_editor_declaration_preparation_is_node_free(self):
        prepared = run(["bun", "--bun", "scripts/prepare-vue-lint-types.mjs"])
        self.assertEqual(prepared.returncode, 0, prepared.stdout + prepared.stderr)
        output = ROOT / "node_modules/.cache/fvoci-vue-lint/types"
        app = output / "apps/web/src/vue/App.vue.d.ts"
        self.assertIn('import("vue").DefineComponent', app.read_text())
        self.assertEqual(len(list((output / "packages/editor/src/vue").glob("*.vue.d.ts"))), 8)
        result = run([*ESLINT, "packages/editor/src/vue/node-views.ts", "--max-warnings=0", "--format=json"])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # Use the real main.ts import location and rootDirs, without its unrelated
        # source diagnostics masking whether createApp receives a genuine type.
        source = '''import { createApp } from "vue";
import App from "./App.vue";
export const app = createApp(App);'''
        result = run([*ESLINT, "--stdin", "--stdin-filename", "apps/web/src/vue/main.ts",
                      "--max-warnings=0", "--format=json"], input=source)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_multiple_declaration_projects_fail_closed_together(self):
        app = self.source("ProofApp.vue", '''<script setup lang="ts">
defineProps<{ label: string }>();
</script><template><p>{{ label }}</p></template>''')
        editor = self.source("ProofEditor.vue", '''<script setup lang="ts">
import type { NodeViewProps } from "@tiptap/vue-3";
defineProps<NodeViewProps>();
</script><template><span /></template>''')
        output = self.directory / "combined"
        projects = []
        for name, component, config in [
            ("editor", editor, "packages/editor/tsconfig.vue.json"),
            ("web", app, "apps/web/tsconfig.vue.json"),
        ]:
            project = self.directory / f"tsconfig.{name}.emit.json"
            project.write_text(json.dumps({
                "extends": str(ROOT / config),
                "compilerOptions": {"rootDir": str(self.directory), "incremental": False, "composite": False},
                "include": [str(component)], "exclude": [],
            }))
            projects.append(str(project))
        prepare = self.source("prepare-combined.mjs", f'''import {{ prepareVueLintTypes }} from {json.dumps(str(ROOT / "scripts/prepare-vue-lint-types.mjs"))};
prepareVueLintTypes({json.dumps(projects)}, {json.dumps(str(output))});''')
        emitted = run(["bun", "--bun", str(prepare)])
        self.assertEqual(emitted.returncode, 0, emitted.stdout + emitted.stderr)
        for name in ["ProofApp.vue.d.ts", "ProofEditor.vue.d.ts"]:
            self.assertTrue((output / name).is_file(), name)
        consumer = self.source("ProofCombined.ts", '''import { createApp } from "vue";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
import ProofApp from "./ProofApp.vue";
import ProofEditor from "./ProofEditor.vue";
export const app = createApp(ProofApp);
export const renderer = VueNodeViewRenderer(ProofEditor);
export function read(value: InstanceType<typeof ProofApp>): string { return value.$props.label; }''')
        lint_project = self.directory / "tsconfig.combined.json"
        lint_project.write_text(json.dumps({
            "extends": str(ROOT / "apps/web/tsconfig.eslint.json"),
            "compilerOptions": {"rootDirs": [str(self.directory), str(output)]},
            "include": [str(consumer)], "exclude": [],
        }))
        args = [*ESLINT, str(consumer), "--max-warnings=0", "--format=json",
                "--parser-options", json.dumps({"project": [str(lint_project)]})]
        typed = run(args)
        self.assertEqual(typed.returncode, 0, typed.stdout + typed.stderr)
        # Reject the second project after the first has emitted fresh output.
        app.write_text('<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>')
        failed = run(["bun", "--bun", str(prepare)])
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("TS2322", failed.stdout + failed.stderr)
        self.assertFalse(output.exists())
        missing = run(args)
        self.assertNotEqual(missing.returncode, 0)
        messages = [m for f in json.loads(missing.stdout) for m in f["messages"]]
        self.assertEqual(sum(m["ruleId"] == "@typescript-eslint/no-unsafe-argument" for m in messages), 2, messages)

    def test_generated_sfc_types_and_failed_refresh_have_no_waiver(self):
        component = self.source("ProofGenerated.vue", '''<script setup lang="ts">
defineProps<{ label: string }>();
</script><template><p>{{ label }}</p></template>''')
        output = self.directory / "generated"
        project = self.directory / "tsconfig.emit.json"
        project.write_text(json.dumps({
            "extends": str(ROOT / "apps/web/tsconfig.vue.json"),
            "compilerOptions": {"rootDir": str(self.directory), "incremental": False, "composite": False},
            "include": [str(component)], "exclude": [],
        }))
        prepare = self.source("prepare.mjs", f'''import {{ prepareVueLintTypes }} from {json.dumps(str(ROOT / "scripts/prepare-vue-lint-types.mjs"))};
prepareVueLintTypes({json.dumps(str(project))}, {json.dumps(str(output))});''')
        emitted = run(["bun", "--bun", str(prepare)])
        self.assertEqual(emitted.returncode, 0, emitted.stdout + emitted.stderr)
        declaration = output / "ProofGenerated.vue.d.ts"
        self.assertTrue(declaration.is_file())
        consumer = self.source("ProofGeneratedConsumer.ts", '''import ProofGenerated from "./ProofGenerated.vue";
export function read(value: InstanceType<typeof ProofGenerated>): string { return value.$props.label; }''')
        lint_project = self.directory / "tsconfig.generated.json"
        lint_project.write_text(json.dumps({
            "extends": str(ROOT / "apps/web/tsconfig.eslint.json"),
            "compilerOptions": {"rootDirs": [str(self.directory), str(output)]},
            "include": [str(consumer)], "exclude": [],
        }))
        args = [*ESLINT, str(consumer), "--max-warnings=0", "--format=json",
                "--parser-options", json.dumps({"project": [str(lint_project)]})]
        typed = run(args)
        self.assertEqual(typed.returncode, 0, typed.stdout + typed.stderr)
        consumer.write_text(consumer.read_text().replace("value.$props.label", "value.$props.label.missing"))
        invalid = run(["bun", "--bun", str(ROOT / "node_modules/.bin/tsc"), "--noEmit", "-p", str(lint_project)])
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn("TS2339", invalid.stdout + invalid.stderr)
        consumer.write_text(consumer.read_text().replace("value.$props.label.missing", "value.$props.label"))
        # Real unsafe props remain unsafe even through compiler-generated types.
        component.write_text(component.read_text().replace("label: string", "label: any"))
        emitted = run(["bun", "--bun", str(prepare)])
        self.assertEqual(emitted.returncode, 0, emitted.stdout + emitted.stderr)
        unsafe = run(args)
        self.assertNotEqual(unsafe.returncode, 0)
        self.assertIn("@typescript-eslint/no-unsafe-return", {m["ruleId"] for f in json.loads(unsafe.stdout) for m in f["messages"]})
        # Failed refresh clears stale successful output before checking source.
        component.write_text('<script setup lang="ts">defineProps<{ label: string }>();</script><template><p>{{ label.toFixed() }}</p></template>')
        failed = run(["bun", "--bun", str(prepare)])
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("TS2551", failed.stdout + failed.stderr)
        self.assertFalse(declaration.exists())
        missing = run(args)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("@typescript-eslint/no-unsafe-return", {m["ruleId"] for f in json.loads(missing.stdout) for m in f["messages"]})

    def test_unprepared_sfc_import_has_no_fake_fallback(self):
        self.source("ProofNode.vue", '''<script setup lang="ts">
import type { NodeViewProps } from "@tiptap/vue-3";
defineProps<NodeViewProps>();
</script><template><span /></template>''')
        source = '''import ProofNode from "./ProofNode.vue";
import { VueNodeViewRenderer } from "@tiptap/vue-3";
export const renderer = VueNodeViewRenderer(ProofNode);'''
        # Document the plain-TS program's SFC import gap without a shim/waiver.
        self.assert_rule("ProofSfcImport.ts", source, "@typescript-eslint/no-unsafe-argument")
        for config in ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]:
            typed = self.compiler(config, source, "ts", "vue-tsc")
            self.assertEqual(typed.returncode, 0, typed.stdout + typed.stderr)
            broken = self.compiler(config, source + '\nexport const invalid: number = "bad";', "ts", "vue-tsc")
            self.assertNotEqual(broken.returncode, 0)
            self.assertIn("TS2322", broken.stdout + broken.stderr)

    def test_y_text_declared_string_contract_without_rule_allowance(self):
        source = 'import * as Y from "yjs"; export function text(value: Y.Text): string { return value.toJSON(); }'
        result, report = self.lint("ProofYText.ts", source)
        self.assertEqual(result.returncode, 0, report)
        self.assert_rule("ProofYTextInherited.ts", source.replace("value.toJSON()", "value.toString()"), "@typescript-eslint/no-base-to-string")
        self.assert_rule("ProofObjectString.ts", 'export function text(value: object): string { return value.toString(); }', "@typescript-eslint/no-base-to-string")

    def test_formatter_keeps_import_order_text_and_tailwind(self):
        source = 'import "./z.css";\nimport "./a.css";\nexport const value = 1;\n'
        result = run([*PRETTIER, "--stdin-filepath", "order.ts"], input=source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(result.stdout.index('"./z.css"'), result.stdout.index('"./a.css"'))
        source = '<template><p>before <strong>middle</strong> after</p><pre>  keep\n spacing </pre></template>'
        result = run([*PRETTIER, "--stdin-filepath", "text.vue"], input=source)
        self.assertIn('before <strong>middle</strong> after', result.stdout)
        self.assertIn('  keep\n spacing ', result.stdout)
        source = '@import "tailwindcss" source(none);\n@source "./";\n@theme { --color-brand: #123456; }\n@utility proof { @apply flex; }\n'
        result = run([*PRETTIER, "--stdin-filepath", "proof.css"], input=source)
        self.assertEqual(result.returncode, 0, result.stderr)




def run(args, **kwargs):
    # Bun 1.4.2 + ESLint's exit path can truncate >64KiB piped stdout.
    # A regular file preserves the complete print-config/JSON evidence.
    with tempfile.TemporaryFile(mode="w+") as output:
        result = subprocess.run(args, cwd=ROOT, text=True, stdout=output,
                                stderr=subprocess.PIPE, **kwargs)
        output.seek(0)
        return subprocess.CompletedProcess(args, result.returncode, output.read(), result.stderr)

CHILD = '<script setup lang="ts">\ndefineProps<{ label: string }>();\ndefineSlots<{ default(props: { row: { id: number; label: string } }): unknown }>();\n</script>\n<template><slot :row="{ id: 1, label }" /></template>\n'

VALID = '<script setup lang="ts">\nimport UButton from "@nuxt/ui/components/Button.vue";\nimport { ref } from "vue";\nimport ProofChild from "./ProofChild.vue";\nimport type { ImportedProps } from "./props";\n\ndefineProps<ImportedProps & { enabled: boolean }>();\nconst emit = defineEmits<{ select: [id: number] }>();\ndefineSlots<{ default(props: { value: string }): unknown }>();\nconst model = ref("");\nconst rows = [{ id: 1, label: "one" }];\nfunction select(id: number): void { emit("select", id); }\n</script>\n<template>\n  <section v-if="enabled" class="flex gap-2 hover:bg-teal-50">\n    <label>Search<input v-model="model" name="search" /></label>\n    <ProofChild :label="title">\n      <template #default="{ row }"><span>{{ row.label }}</span></template>\n    </ProofChild>\n    <UButton v-for="row in rows" :key="row.id" type="button" @click="select(row.id)">{{ row.label }}</UButton>\n    <slot :value="model" />\n  </section>\n</template>\n<style scoped>\n.host :deep(.child) { color: red; }\n.host :slotted(span) { color: blue; }\n</style>\n'

if __name__ == "__main__":
    unittest.main(verbosity=2)
