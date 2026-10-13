import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";
import { renderToString } from "vue/server-renderer";
import * as Vue from "vue";
import { t } from "@fvoci/i18n";
import * as api from "@/lib/api";
import * as catalog from "./settings-catalog";
import * as model from "./settings-instance-model";
import * as fieldClasses from "../../vue/features/settings/field-classes";
import * as form from "../../vue/composables/useZodForm";
import { compiledComponent, evaluate, record, renderFunction } from "./compiled-component-test";

const filename = new URL("../../vue/features/settings/InstanceSettingsView.vue", import.meta.url)
  .pathname;
const { descriptor } = parse(readFileSync(filename, "utf8"), { filename });
const script = compileScript(descriptor, { id: "instance-settings-issue" });
assert.ok(descriptor.template, "actual component has a template");
const template = compileTemplate({
  source: descriptor.template.content,
  filename,
  id: "instance-settings-issue",
  compilerOptions: { bindingMetadata: script.bindings },
});
assert.deepEqual(template.errors, []);

// Compile the actual Vue script and template; only child components are stubbed.
// The schema, the draft and the issue line are the component's own.
const stub = Vue.defineComponent({ setup: () => () => null });
const imports = {
  vue: Vue,
  "@fvoci/i18n": { t },
  "@nuxt/ui/components/Button.vue": { default: stub },
  "../../components/ConfirmAction.vue": { default: stub },
  "../../components/QueryError.vue": { default: stub },
  "../../components/QueryLoading.vue": { default: stub },
  "../../composables/useZodForm": form,
  "@/lib/api": api,
  "@/features/settings/settings-catalog": catalog,
  "@/features/settings/settings-instance-model": model,
  "./field-classes": fieldClasses,
  "@/features/settings/settings-shell.css": {},
};

const LOGO = {
  key: "01900000-0000-7000-8000-000000000001",
  sha256: "a".repeat(64),
  mime: "image/png",
};
const BRANDING = { name: "FVOCI", smtpFromDisplay: null, loginBrandText: null, logo: LOGO };

/** Render with branding edited through the component's own leaf setter. */
async function renderEdited(values: Record<string, unknown>, leaf: string, next: unknown) {
  const component = compiledComponent(evaluate(script.content, imports).default);
  const setup = component.setup;
  component.setup = (props, context) => {
    const state = setup(props, context);
    const setLeaf = record(state).setLeaf;
    assert.equal(typeof setLeaf, "function", "actual setup exposes the leaf setter");
    Reflect.apply(setLeaf as (...args: unknown[]) => unknown, undefined, ["branding", leaf, next]);
    return state;
  };
  component.render = renderFunction(evaluate(template.code, imports).render);
  return renderToString(
    Vue.createSSRApp(component, {
      data: {
        attachmentTransfer: undefined,
        eeFeatures: [],
        envApplied: [],
        overridden: [],
        restartRequired: [],
        values,
        version: 1,
      },
      loading: false,
      error: null,
      pending: false,
      onSave: () => Promise.resolve(),
      onAsset: () => Promise.resolve(),
    }),
  );
}

function issueLines(html: string): string[] {
  return [...html.matchAll(/<p class="text-xs text-error" role="alert">([^<]*)<\/p>/g)].map(
    (match) =>
      (match[1] ?? "")
        .replaceAll("&quot;", '"')
        .replaceAll("&#39;", "'")
        .replaceAll("&lt;", "<")
        .replaceAll("&gt;", ">")
        .replaceAll("&amp;", "&")
        .trim(),
  );
}

for (const [label, leaf, next, values, code, path] of [
  ["min", "name", model.textFieldValue(" "), { branding: BRANDING }, "too_small", "name"],
  ["max", "name", "x".repeat(81), { branding: BRANDING }, "too_big", "name"],
  [
    "strict",
    "name",
    "FVOCI 2",
    { branding: { ...BRANDING, logo: { ...LOGO, extra: true } } },
    "unrecognized_keys",
    "logo",
  ],
] as const) {
  await test(`an edited setting shows its first issue path and message (${label})`, async () => {
    const draft = model.withLeaf(values.branding, leaf, next);
    const parsed = catalog.SETTINGS_CATALOG.branding.schema.safeParse(draft);
    assert.equal(parsed.success, false);
    const issue = parsed.error.issues[0];
    assert.equal(issue?.code, code);
    assert.equal(issue.path.join("."), path);
    // The line is the schema's own issue; the library's wording is not pinned.
    assert.deepEqual(issueLines(await renderEdited(values, leaf, next)), [
      `${path}: ${issue.message}`,
    ]);
    assert.ok(issue.message.trim().length > 0, "a non-empty message follows the path");
  });
}

await test("an unedited or valid setting shows no issue line", async () => {
  assert.deepEqual(issueLines(await renderEdited({ branding: BRANDING }, "name", "FVOCI 2")), []);
});
