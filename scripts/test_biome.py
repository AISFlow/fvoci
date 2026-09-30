#!/usr/bin/env python3
"""Exercise the locked tools, without editing or formatting product sources."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
BIOME = ["bun", "--bun", str(ROOT / "node_modules/@biomejs/biome/bin/biome")]
ENV = {key: value for key, value in os.environ.items() if key != "BIOME_BINARY"}

CHILD = '''<script setup lang="ts">
defineProps<{ label: string }>();
defineSlots<{ default(props: { row: { id: number; label: string } }): unknown }>();
</script>
<template><slot :row="{ id: 1, label }" /></template>
'''

VALID = '''<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import ProofChild from "./ProofChild.vue";
import type { ImportedProps } from "./props";

defineProps<ImportedProps & { enabled: boolean }>();
const emit = defineEmits<{ select: [id: number] }>();
defineSlots<{ default(props: { value: string }): unknown }>();
const model = ref("");
const rows = [{ id: 1, label: "one" }];
function select(id: number): void { emit("select", id); }
</script>
<template>
  <section v-if="enabled" class="flex gap-2 hover:bg-teal-50">
    <label>Search<input v-model="model" name="search" /></label>
    <ProofChild :label="title">
      <template #default="{ row }"><span>{{ row.label }}</span></template>
    </ProofChild>
    <UButton v-for="row in rows" :key="row.id" type="button" @click="select(row.id)">{{ row.label }}</UButton>
    <slot :value="model" />
  </section>
</template>
<style scoped>
.host :deep(.child) { color: red; }
.host :slotted(span) { color: blue; }
</style>
'''


def run(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, env=ENV, text=True, capture_output=True, **kwargs)


class BiomeSupport(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        version = run([*BIOME, "--version"])
        if version.returncode or version.stdout.strip() != "Version: 2.5.14":
            raise AssertionError(version.stdout + version.stderr)
        cls.temp = tempfile.TemporaryDirectory(prefix="biome-proof-", dir=ROOT / "scripts")
        cls.directory = Path(cls.temp.name)
        (cls.directory / "props.ts").write_text('export interface ImportedProps { title: string }\n')
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
        result = run([*BIOME, "lint", str(path), "--colors=off", "--error-on-warnings",
                      "--reporter=json", "--max-diagnostics=none", *options])
        report = json.loads(result.stdout)
        return result, report

    def assert_rule(self, name, source, rule):
        result, report = self.lint(name, source)
        self.assertNotEqual(result.returncode, 0, report)
        self.assertIn(rule, {entry["category"] for entry in report["diagnostics"]}, report)

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

    def test_full_valid_sfc_and_actual_nuxt_ui_imports(self):
        result, report = self.lint("ProofValid.vue", VALID)
        self.assertEqual(result.returncode, 0, report)
        path = self.directory / "ProofValid.vue"
        formatted = run([*BIOME, "format", "--stdin-file-path", str(path), "--colors=off"], input=VALID)
        self.assertEqual(formatted.returncode, 0, formatted.stderr)
        self.assertNotEqual(formatted.stdout, VALID)
        # All three SFC sections participate; do not infer template support from JS alone.
        self.assertIn("color: red;\n", formatted.stdout)
        self.assertIn(':key="row.id"', formatted.stdout)
        self.assertIn('emit("select", id);\n', formatted.stdout)
        path.write_text(formatted.stdout)
        checked = run([*BIOME, "ci", "--error-on-warnings", "--colors=off", str(path)])
        self.assertEqual(checked.returncode, 0, checked.stdout + checked.stderr)
        for config in ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]:
            with self.subTest(config=config):
                result = self.compiler(config, VALID)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_scoped_slots_and_imported_props_gap_is_reproduced(self):
        source = '''<script setup lang="ts">
import ProofChild from "./ProofChild.vue";
import type { ImportedProps } from "./props";
defineProps<ImportedProps & { enabled: boolean }>();
</script>
<template><ProofChild :label="title" v-slot="{ row }"><span>{{ row.label }}</span></ProofChild></template>
'''
        valid, report = self.lint("ProofGap.vue", source)
        self.assertEqual(valid.returncode, 0, report)
        broken, report = self.lint("ProofGap.vue", source, "--only=correctness/noUndeclaredVariables")
        # Remove the override only after a reviewed upgrade makes this negative repro pass.
        self.assertNotEqual(broken.returncode, 0, report)
        self.assertIn("lint/correctness/noUndeclaredVariables",
                      {entry["category"] for entry in report["diagnostics"]})

    def test_unused_script_names_and_imports_still_fail(self):
        self.assert_rule("ProofUnused.vue", '<script setup lang="ts">const unused = 1;</script><template><p>ok</p></template>',
                         "lint/correctness/noUnusedVariables")
        self.assert_rule("ProofUnusedImport.vue", '<script setup lang="ts">import { ref } from "vue";</script><template><p>ok</p></template>',
                         "lint/correctness/noUnusedImports")

    def test_dynamic_slot_name_is_an_unresolved_biome_gap(self):
        source = '''<script setup lang="ts">
import ProofChild from "./ProofChild.vue";
const slotName = "default";
</script>
<template><ProofChild label="ok"><template #[slotName]="{ row }"><span>{{ row.label }}</span></template></ProofChild></template>
'''
        result, report = self.lint("ProofDynamic.vue", source)
        self.assertNotEqual(result.returncode, 0, report)
        self.assertIn("lint/correctness/noUnusedVariables",
                      {entry["category"] for entry in report["diagnostics"]})
        for config in ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]:
            with self.subTest(config=config):
                result = self.compiler(config, source)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                result = self.compiler(config, source.replace("#[slotName]", "#[missingSlotName]"))
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("missingSlotName", result.stdout + result.stderr)

    def test_template_correctness_a11y_security_and_parser_fail(self):
        cases = [
            ("ProofFor.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-for="row in rows">{{ row }}</p></template>', "lint/correctness/useVueVForKey"),
            ("ProofForSyntax.vue", '<template><p v-for="item">bad</p></template>', "lint/nursery/useVueValidVFor"),
            ("ProofIfFor.vue", '<script setup lang="ts">const rows = [1];</script><template><p v-if="true" v-for="row in rows" :key="row">{{ row }}</p></template>', "lint/correctness/noVueVIfWithVFor"),
            ("ProofIf.vue", '<template><p v-if>bad</p></template>', "lint/correctness/useVueValidVIf"),
            ("ProofAlt.vue", '<template><img src="x.png" /></template>', "lint/a11y/useAltText"),
            ("ProofSecurity.ts", 'export const value = eval("1");', "lint/security/noGlobalEval"),
            ("ProofScriptUrl.vue", '<template><a href="javascript:alert(1)">bad</a></template>', "lint/security/noScriptUrl"),
            ("ProofParse.vue", '<script setup lang="ts">const = ;</script><template><p>bad</p></template>', "parse"),
            ("ProofAny.ts", 'export const value: any = 1;', "lint/suspicious/noExplicitAny"),
            ("ProofPromise.ts", 'export async function save(): Promise<number> { return 1; }\nsave();', "lint/nursery/noFloatingPromises"),
        ]
        for name, source, rule in cases:
            with self.subTest(name=name):
                self.assert_rule(name, source, rule)

    def test_script_and_template_types_fail_in_web_and_editor(self):
        for config in ["apps/web/tsconfig.vue.json", "packages/editor/tsconfig.vue.json"]:
            for source, expected in [
                ('<script setup lang="ts">const value: number = "bad";</script><template><p>{{ value }}</p></template>', "TS2322"),
                ('<script setup lang="ts">const value = 1;</script><template><p>{{ value.toUpperCase() }}</p></template>', "TS2339"),
                ('<template><p>{{ missingTemplateName }}</p></template>', "TS2339"),
                ('<script setup lang="ts">const value = missingScriptName;</script><template><p>{{ value }}</p></template>', "TS2304"),
                ('<template><UnknownComponent /></template>', "UnknownComponent"),
                ('<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild :label="1" /></template>', "TS2322"),
                ('<script setup lang="ts">const emit = defineEmits<{ select: [id: number] }>(); emit("select", "bad");</script><template><p>bad emit</p></template>', "TS2345"),
                ('<script setup lang="ts">import ProofChild from "./ProofChild.vue";</script><template><ProofChild label="ok" v-slot="{ row }"><p>{{ row.missing }}</p></ProofChild></template>', "TS2339"),
            ]:
                with self.subTest(config=config, expected=expected):
                    result = self.compiler(config, source)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn(expected, result.stdout + result.stderr)
        for config in ["apps/web/tsconfig.app.json", "packages/editor/tsconfig.json"]:
            with self.subTest(config=config):
                result = self.compiler(config, 'export const value: number = "bad";', "ts", "tsc")
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("TS2322", result.stdout + result.stderr)

    def test_nuxt_autoimport_is_a_compiler_gap_without_registration(self):
        # Biome accepts template component names, but cannot prove global registration.
        source = '<template><UButton type="button">ok</UButton></template>'
        result, report = self.lint("ProofAutoImport.vue", source)
        self.assertEqual(result.returncode, 0, report)
        result = self.compiler("apps/web/tsconfig.vue.json", source)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("UButton", result.stdout + result.stderr)

    def test_tailwind_directives(self):
        source = '@import "tailwindcss" source(none);\n@source "./";\n@theme { --color-brand: #123456; }\n@utility proof { @apply flex; }\n'
        result, report = self.lint("proof.css", source)
        self.assertEqual(result.returncode, 0, report)

    def test_vue_composables_and_react_rules_are_scoped_to_their_framework(self):
        source = '<script setup lang="ts">import { useTemplateRef } from "vue"; const input = useTemplateRef<HTMLInputElement>("input");</script><template><input ref="input" :disabled="!!input" name="proof" aria-label="proof" /></template>'
        result, report = self.lint("ProofComposable.vue", source)
        self.assertEqual(result.returncode, 0, report)
        # Match editor's nearest-package React dependency; root itself has none.
        react = self.directory / "react"
        react.mkdir()
        (react / "package.json").write_text(json.dumps({"dependencies": {"react": "19.3.0"}}))
        self.assert_rule("react/ProofReact.tsx", 'import { useState } from "react"; export function ProofReact() { if (Math.random() > 0.5) { useState(0); } return <p>ok</p>; }',
                         "lint/correctness/useHookAtTopLevel")

    def test_ci_rejects_warnings_and_ignores_external_binary_override(self):
        path = self.source("ProofWarning.ts", 'export function read(unused: string): number { return 1; }\n')
        formatted = run([*BIOME, "format", "--stdin-file-path", str(path), "--colors=off"], input=path.read_text())
        self.assertEqual(formatted.returncode, 0, formatted.stderr)
        path.write_text(formatted.stdout)
        result = run(["bash", "scripts/lint-web.sh", str(path), "--colors=off",
                      "--reporter=json", "--max-diagnostics=none"])
        report = json.loads(result.stdout)
        self.assertEqual(report["summary"]["errors"], 0, report)
        self.assertEqual(report["summary"]["warnings"], 1, report)
        self.assertNotEqual(result.returncode, 0, report)
        result = subprocess.run(["bash", "scripts/lint-web.sh", "biome.json", "--colors=off"],
                                cwd=ROOT, text=True, capture_output=True,
                                env={**ENV, "BIOME_BINARY": "/nonexistent-external-biome"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_format_keeps_side_effect_import_order_and_sensitive_text(self):
        source = 'import "./z.css";\nimport "./a.css";\nexport const value = 1;\n'
        result = run([*BIOME, "check", "--stdin-file-path", str(self.directory / "order.ts"),
                      "--write", "--colors=off"], input=source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(result.stdout.index('"./z.css"'), result.stdout.index('"./a.css"'))
        source = '<template><p>before <strong>middle</strong> after</p><pre>  keep\n spacing </pre></template>'
        result = run([*BIOME, "format", "--stdin-file-path", str(self.directory / "text.vue"),
                      "--colors=off"], input=source)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('before <strong>middle</strong> after', result.stdout)
        self.assertIn('  keep\n spacing ', result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
