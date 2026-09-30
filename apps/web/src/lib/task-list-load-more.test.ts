import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import * as Vue from "vue";
import { compileScript, parse } from "vue/compiler-sfc";
import { renderToString } from "vue/server-renderer";
import { t } from "@fvoci/i18n";
import { visibleTaskStatusSections } from "../features/tasks/task-list-page.ts";
import { taskTypeLabel } from "../features/tasks/task-types.ts";
import { formatDisplayId, itemPath } from "./href.ts";

// Render the retained TaskList SFC, including its real rejection/pending
// branch. The design-system button and unused links only provide SSR hosts.
function taskList() {
  const { descriptor } = parse(
    readFileSync(new URL("../vue/features/tasks/TaskList.vue", import.meta.url), "utf8"),
  );
  let source = compileScript(descriptor, {
    id: "load-more-contract",
    inlineTemplate: true,
  }).content;
  const button = Vue.defineComponent({
    setup:
      (_props, { attrs, slots }) =>
      () =>
        Vue.h("button", attrs, slots.default?.()),
  });
  const modules: Record<string, unknown> = {
    vue: Vue,
    "@fvoci/i18n": { t },
    "@nuxt/ui/components/Button.vue": { default: button },
    "@/features/tasks/task-list-page": { visibleTaskStatusSections },
    "@/features/tasks/task-types": { taskTypeLabel },
    "@/lib/href": { formatDisplayId, itemPath },
    "../../components/AppLink.vue": { default: Vue.defineComponent({ render: () => null }) },
  };
  const ast = ts.createSourceFile(
    "task-list.ts",
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const bindings: string[] = [];
  for (const statement of [...ast.statements].reverse()) {
    if (!ts.isImportDeclaration(statement)) continue;
    const clause = statement.importClause;
    const specifier = statement.moduleSpecifier.getText(ast).slice(1, -1);
    if (clause && !clause.isTypeOnly) {
      if (clause.name)
        bindings.push(`const ${clause.name.text} = modules[${JSON.stringify(specifier)}].default;`);
      if (clause.namedBindings && ts.isNamedImports(clause.namedBindings)) {
        for (const binding of clause.namedBindings.elements)
          if (!binding.isTypeOnly)
            bindings.push(
              `const ${binding.name.text} = modules[${JSON.stringify(specifier)}][${JSON.stringify(binding.propertyName?.text ?? binding.name.text)}];`,
            );
      }
    }
    source = source.slice(0, statement.getFullStart()) + source.slice(statement.end);
  }
  return new Function(
    "modules",
    new Bun.Transpiler({ loader: "ts" }).transformSync(
      bindings.join("\n") + source.replace("export default", "return"),
    ),
  )(modules);
}

test("rejected second page shows a Korean alert and keeps load-more usable in the retained Vue list", async () => {
  const component = taskList();
  const props = {
    slug: "acme",
    projectKey: "OPS",
    items: [],
    statusCounts: [],
    statuses: [],
    canCreate: false,
    defaultStatusId: null,
    hasMore: true,
    loadMoreError: t("task.list.loadMoreFailed"),
    loadMorePending: false,
  };
  const html = await renderToString(Vue.createSSRApp(component, props));
  assert.match(html, /role="alert"/);
  assert.match(html, /태스크를 더 불러오지 못했습니다\. 다시 시도해 주세요\./);
  assert.match(html, /더 보기/);
  assert.doesNotMatch(html, /\sdisabled(=|>|\s)/);
  const pending = await renderToString(
    Vue.createSSRApp(component, { ...props, loadMorePending: true }),
  );
  assert.match(pending, /\sdisabled(=|>|\s)/);
});
