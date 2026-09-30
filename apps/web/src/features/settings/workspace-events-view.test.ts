import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";
import { renderToString } from "vue/server-renderer";
import ts from "typescript";
import * as Vue from "vue";
import { t } from "@fvoci/i18n";
import * as api from "@/lib/api";
import * as datetime from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import { workspaceEventsQuery } from "@/lib/queries/workspace";
import type { components } from "@/generated/api";
import { flattenEventPages } from "../../vue/features/settings/events-pages.ts";

const filename = new URL("../../vue/features/settings/WorkspaceEventsSection.vue", import.meta.url)
  .pathname;
const { descriptor } = parse(readFileSync(filename, "utf8"), { filename });
const script = compileScript(descriptor, { id: "events-contract" });
const template = compileTemplate({
  source: descriptor.template!.content,
  filename,
  id: "events-contract",
  compilerOptions: { bindingMetadata: script.bindings },
});
assert.deepEqual(template.errors, []);

// Compile the actual Vue script and template; substitute query snapshots and
// leaf controls only. Policy computations, event handlers and rendering are real.
function evaluate(code: string, imports: Record<string, unknown>) {
  const js = ts.transpileModule(code, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const module = { exports: {} as any };
  new Function("require", "module", "exports", js)(
    (name: string) => {
      assert.ok(name in imports, `unmapped component import: ${name}`);
      return imports[name];
    },
    module,
    module.exports,
  );
  return module.exports;
}

const button = Vue.defineComponent({
  setup:
    (_props, { attrs, slots }) =>
    () =>
      Vue.h("button", attrs, slots.default?.()),
});
function event(
  id: string,
  verb: string,
  createdAt: string,
): components["schemas"]["WorkspaceEventOutput"] {
  return {
    id,
    verb,
    workspaceId: "01900000-0000-7000-8000-000000000001",
    actorUserId: null,
    targetType: null,
    targetId: null,
    payload: {},
    channel: "web",
    createdAt,
  };
}

async function render(
  overrides: {
    pages?: { items: components["schemas"]["WorkspaceEventOutput"][] }[];
    timeZone?: string | null;
    loading?: boolean;
    error?: unknown;
    hasMore?: boolean;
    loadingMore?: boolean;
    nextPageFailed?: boolean;
  } = {},
) {
  const {
    pages = [],
    timeZone = "Asia/Seoul",
    loading = false,
    error = null,
    hasMore = false,
    loadingMore = false,
    nextPageFailed = false,
  } = overrides;
  const imports = {
    vue: Vue,
    "@fvoci/i18n": { t },
    "@/lib/api": api,
    "@/lib/datetime": datetime,
    "@/lib/queries": { meQuery },
    "@/lib/queries/workspace": { workspaceEventsQuery },
    "./events-pages": { flattenEventPages },
    "@/features/settings/settings-shell.css": {},
    "@nuxt/ui/components/Button.vue": { default: button },
    "@tanstack/vue-query": {
      useQuery: () => ({ data: Vue.ref({ timezone: timeZone }) }),
      useInfiniteQuery: (options: () => ReturnType<typeof workspaceEventsQuery>) => {
        const query = options();
        assert.deepEqual(query.queryKey, [
          "workspace-events",
          "01900000-0000-7000-8000-000000000001",
        ]);
        assert.equal(query.retry, false);
        return {
          data: Vue.ref({ pages }),
          isLoading: Vue.ref(loading),
          isError: Vue.ref(Boolean(error)),
          error: Vue.ref(error),
          isFetchNextPageError: Vue.ref(nextPageFailed),
          hasNextPage: Vue.ref(hasMore),
          isFetchingNextPage: Vue.ref(loadingMore),
          refetch: () => {},
          fetchNextPage: () => {},
        };
      },
    },
  };
  const component = evaluate(script.content, imports).default;
  component.render = evaluate(template.code, imports).render;
  return renderToString(
    Vue.createSSRApp(component, { workspaceId: "01900000-0000-7000-8000-000000000001" }),
  );
}

test("Vue event rows show verb and Seoul-local time under the Korean activity title", async () => {
  const html = await render({
    pages: [
      { items: [event("a", "workspace.created", "2026-09-27T15:30:00.000Z")] },
      { items: [event("b", "project.updated", "2026-09-28T01:05:00.000Z")] },
    ],
  });
  assert.match(html, /<h2[^>]*>활동<\/h2>/);
  assert.ok(
    html.indexOf("workspace.created") < html.indexOf("project.updated"),
    "server order kept across pages",
  );
  assert.match(html, /2026\. 09\. 28\. 00:30/);
  assert.match(html, /2026\. 09\. 28\. 10:05/);
  assert.doesNotMatch(html, /활동이 없습니다/);
  assert.doesNotMatch(html, /더 보기/, "no load-more without a next cursor");
  assert.match(
    await render({
      pages: [{ items: [event("a", "workspace.created", "2026-09-27T15:30:00.000Z")] }],
      timeZone: null,
    }),
    /2026\. 09\. 28\. 00:30/,
  );
});

test("Vue empty, loading and first-page error states remain distinct", async () => {
  assert.match(await render(), /활동이 없습니다/);
  const loading = await render({ loading: true });
  assert.match(loading, /role="status"[^>]*>불러오는 중…/);
  assert.doesNotMatch(loading, /활동이 없습니다/);
  const failed = await render({ error: new Error("transport failed"), hasMore: true });
  assert.match(failed, /role="alert"[^>]*>불러오지 못했습니다\./);
  assert.match(failed, /다시 시도/);
  assert.doesNotMatch(failed, /활동이 없습니다/);
  assert.doesNotMatch(failed, /더 보기/);
});

test("Vue next cursor offers load-more; later-page failure preserves rows and permits retry", async () => {
  const pages = [{ items: [event("a", "task.created", "2026-09-28T00:00:00.000Z")] }];
  const more = await render({ pages, hasMore: true });
  assert.match(more, /더 보기/);
  assert.doesNotMatch(more, /\sdisabled(=|>|\s)/);
  const pending = await render({ pages, hasMore: true, loadingMore: true });
  assert.match(pending, /disabled/);
  const failed = await render({
    pages,
    hasMore: true,
    nextPageFailed: true,
    error: new Error("later page failed"),
  });
  assert.match(failed, /task\.created/);
  assert.match(failed, /role="alert"[^>]*>활동을 더 불러오지 못했습니다\. 다시 시도해 주세요\./);
  assert.match(failed, /더 보기/);
  assert.doesNotMatch(failed, /\sdisabled(=|>|\s)/);
  assert.doesNotMatch(
    failed,
    /다시 시도<\/button>/,
    "later-page rejection uses load-more retry, not first-page refetch",
  );
});
