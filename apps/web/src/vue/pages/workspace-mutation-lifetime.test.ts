import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import { parse } from "vue/compiler-sfc";
import { useNavigationError } from "../features/workspace/useNavigationError";
import ts from "typescript";
import * as Vue from "vue";
import * as Query from "@tanstack/vue-query";

function deferred() {
  let controls:
    { resolve: (value: unknown) => void; reject: (reason: unknown) => void } | undefined;
  const promise = new Promise<unknown>((resolve, reject) => {
    controls = { resolve, reject };
  });
  assert.ok(controls, "the Promise executor must initialize its controls synchronously");
  return { promise, ...controls };
}
function record(value: unknown): Record<string, unknown> {
  assert.ok(typeof value === "object" && value !== null, "expected an exported setup object");
  return value as Record<string, unknown>;
}
function callable(value: unknown): value is (...args: unknown[]) => unknown {
  return typeof value === "function";
}
function pageAccess(value: unknown) {
  const exports = record(value);
  function call(name: string, ...args: unknown[]): unknown {
    const method = exports[name];
    assert.ok(callable(method), `expected setup function ${name}`);
    return method(...args);
  }
  function callMutation(name: string, ...args: unknown[]): unknown {
    const mutation = record(exports[name]);
    const method = mutation.mutateAsync;
    assert.ok(callable(method), `expected mutation ${name}`);
    return method(...args);
  }
  function refValue(name: string, member?: string): unknown {
    const value = member === undefined ? exports[name] : record(exports[name])[member];
    assert.ok(Vue.isRef(value), `expected setup ref ${name}/${member ?? "value"}`);
    const result: unknown = value.value;
    return result;
  }
  return { call, callMutation, refValue };
}
// Run the actual SFC setup and actual Vue Query mutations; only HTTP/query data and host DOM are controlled.
const renderer = Vue.createRenderer({
  createElement: () => ({}),
  createText: () => ({}),
  createComment: () => ({}),
  setText() {},
  setElementText() {},
  patchProp() {},
  parentNode: () => null,
  nextSibling: () => null,
  insert() {},
  remove() {},
});
function harness(file: string) {
  const route = Vue.reactive({ params: { slug: "alpha" }, query: {}, hash: "" });
  const workspace = Vue.ref({ id: "A", role: "owner" });
  const me = Vue.ref({ userId: "viewer", sessionId: "session-one", isInstanceAdmin: true });
  const status = Vue.ref("ready");
  const calls: { method: string; workspaceId?: string; key?: readonly unknown[] }[] = [];
  const navigation: string[] = [];
  const queue: ReturnType<typeof deferred>[] = [];
  const client = new Query.QueryClient({ defaultOptions: { mutations: { retry: false } } });
  client.invalidateQueries = (options?: Query.InvalidateQueryFilters) => {
    calls.push({ method: "invalidate", key: options?.queryKey });
    return Promise.resolve();
  };
  const request = async (
    method: string,
    _path: string,
    options: { params: { path: { workspace_id: string } } },
  ) => {
    calls.push({ method, workspaceId: options.params.path.workspace_id });
    const response = queue.shift();
    assert.ok(response, "a controlled response must be queued");
    return response.promise;
  };
  const fakeQuery = () => ({
    data: Vue.ref(undefined),
    isError: Vue.ref(false),
    isSuccess: Vue.ref(false),
    isLoading: Vue.ref(false),
    error: Vue.ref(null),
  });
  const options = () => ({});
  const router = {
    currentRoute: Vue.computed(() => route),
    push: (href: string) => {
      navigation.push(href);
      return Promise.resolve();
    },
    replace: () => Promise.resolve(),
  };
  const injected = {
    ...Vue,
    useNavigationError,
    t: (key: string) => key,
    useRoute: () => route,
    useRouter: () => router,
    useWorkspaceSession: () => ({ workspace, me, status }),
    useQueryClient: Query.useQueryClient,
    useMutation: Query.useMutation,
    useQuery: fakeQuery,
    useInfiniteQuery: fakeQuery,
    projectsQuery: options,
    membersQuery: options,
    meQuery: {},
    treeQuery: options,
    wikiDiscoveryQuery: options,
    trashQuery: options,
    documentTagPoolQuery: options,
    notificationListQuery: options,
    moveDocument: options,
    resolveTreeDrop: options,
    ensureOk: (value: unknown) => value,
    loadErrorMessage: (error: Error) => error.message,
    problemMessage: (error: Error) => error.message,
    ProblemError: Error,
    api: {
      POST: (path: string, input: Parameters<typeof request>[2]) => request("POST", path, input),
      PATCH: (path: string, input: Parameters<typeof request>[2]) => request("PATCH", path, input),
    },
    projectTasksPath: (slug: string, key: string) => `/w/${slug}/${key}/tasks`,
    documentPath: (slug: string, ref: string) => `/w/${slug}/${ref}`,
    notificationHref: (slug: string, item: { id: string }) => `/w/${slug}/${item.id}`,
    followAppHref: (href: string) => router.push(href),
    payloadRecord: options,
    formatPersonName: options,
    notificationMessage: options,
    FALLBACK_TZ: "Asia/Seoul",
  };
  const { descriptor, errors } = parse(readFileSync(new URL(file, import.meta.url), "utf8"));
  assert.deepEqual(errors, [], "the retained SFC must parse");
  assert.ok(descriptor.scriptSetup, "the page must expose script setup");
  let source = descriptor.scriptSetup.content;
  const parsed = ts.createSourceFile(
    "page.ts",
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  for (const statement of [...parsed.statements].reverse()) {
    if (ts.isImportDeclaration(statement))
      source = source.slice(0, statement.getFullStart()) + source.slice(statement.end);
  }
  const exported =
    file === "ProjectsPage.vue"
      ? "onCreate,onClone"
      : file === "WikiPage.vue"
        ? "onCreateDocument"
        : file === "TrashPage.vue"
          ? "onRestore,restore,restoreError"
          : "openItem,perform,readAll,actionError,actionPending";
  let page: unknown;
  let mounted = true;
  const app = renderer.createApp({
    setup() {
      // Evaluate only this retained setup in a fresh context with explicit test dependencies.
      const javascript = new Bun.Transpiler({ loader: "ts" }).transformSync(
        `(() => {${source}\nreturn {${exported}};})()`,
      );
      page = runInNewContext(javascript, injected);
      return () => null;
    },
  });
  app.use(Query.VueQueryPlugin, { queryClient: client });
  app.mount({});
  function detach() {
    if (mounted) {
      app.unmount();
      mounted = false;
    }
  }
  return {
    page: pageAccess(page),
    client,
    calls,
    navigation,
    detach,
    queue() {
      const result = deferred();
      queue.push(result);
      return result;
    },
    transition(kind: string) {
      if (kind === "aba") {
        workspace.value = { id: "B", role: "owner" };
        route.params.slug = "beta";
        workspace.value = { id: "A", role: "owner" };
        route.params.slug = "alpha";
      }
      if (kind === "actor") me.value = { ...me.value, userId: "another-viewer" };
      if (kind === "session") me.value = { ...me.value, sessionId: "session-two" };
      if (kind === "role") workspace.value = { ...workspace.value, role: "guest" };
      if (kind === "guard") {
        status.value = "loading";
        status.value = "ready";
      }
      if (kind === "dispose") detach();
    },
    stop() {
      detach();
      client.clear();
    },
  };
}
async function until(check: () => boolean) {
  for (let step = 0; step < 100; step++) {
    await Vue.nextTick();
    if (check()) return;
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
  assert.fail("mutation did not reach its expected state");
}
const flows = [
  ["ProjectsPage.vue", "create"],
  ["ProjectsPage.vue", "clone"],
  ["WikiPage.vue", "create"],
  ["NotificationsPage.vue", "open"],
] as const;
function invoke(h: ReturnType<typeof harness>, file: string, flow: string) {
  if (file === "ProjectsPage.vue")
    return flow === "create"
      ? h.page.call("onCreate", { key: "OLD" })
      : h.page.call("onClone", "source", { key: "OLD" });
  if (file === "WikiPage.vue") return h.page.call("onCreateDocument");
  return h.page.call("perform", () => h.page.call("openItem", { id: "OLD-7", readAt: null }));
}

await test("late successful mutations cannot navigate after ABA, actor/session/role change, guard retirement or unmount", async () => {
  for (const [file, flow] of flows)
    for (const change of ["aba", "actor", "session", "role", "guard", "dispose"]) {
      const h = harness(file);
      try {
        const response = h.queue();
        const done = invoke(h, file, flow);
        await until(() =>
          h.calls.some((call) => call.method === "POST" || call.method === "PATCH"),
        );
        const mutation = h.client.getMutationCache().getAll()[0];
        h.transition(change);
        response.resolve({ key: "OLD", displayId: "WIKI-7" });
        await done;
        if (mutation) await until(() => mutation.state.status === "success");
        assert.deepEqual(h.navigation, [], `${file}/${flow}/${change}`);
        assert.ok(
          h.calls
            .filter((call) => call.method !== "invalidate")
            .every((call) => call.workspaceId === "A"),
        );
        assert.ok(
          h.calls
            .filter((call) => call.method === "invalidate")
            .every((call) => call.key?.[1] !== "B"),
        );
      } finally {
        h.stop();
      }
    }
});

await test("retired errors do not replace current restore/inbox errors or clear a newer pending action", async () => {
  for (const file of ["TrashPage.vue", "NotificationsPage.vue"])
    for (const change of ["aba", "actor", "session", "role", "guard", "superseded"]) {
      const h = harness(file);
      try {
        const old = h.queue();
        const first =
          file === "TrashPage.vue"
            ? h.page.call("onRestore", { id: "old" })
            : h.page.call("perform", () => h.page.callMutation("readAll", "A"));
        await until(() => h.calls.length === 1);
        const oldMutation = h.client.getMutationCache().getAll()[0];
        assert.ok(oldMutation, "the old request must own a mutation");
        h.transition(change);
        // Notifications deliberately serializes operations within one unchanged lifetime.
        if (change === "superseded" && file === "NotificationsPage.vue") {
          old.reject(new Error("current error"));
          await first;
          assert.equal(h.page.refValue("actionError"), "current error");
          continue;
        }
        const current = h.queue();
        const second =
          file === "TrashPage.vue"
            ? h.page.call("onRestore", { id: "new" })
            : h.page.call("perform", () => h.page.callMutation("readAll", "A"));
        await until(() => h.calls.filter((call) => call.method === "POST").length === 2);
        old.reject(new Error("obsolete error"));
        await first;
        await until(() => oldMutation.state.status === "error");
        assert.equal(
          file === "TrashPage.vue"
            ? h.page.refValue("restoreError")
            : h.page.refValue("actionError"),
          null,
          `${file}/${change}`,
        );
        assert.equal(
          file === "TrashPage.vue"
            ? h.page.refValue("restore", "isPending")
            : h.page.refValue("actionPending"),
          true,
        );
        current.resolve({});
        await second;
        await until(() => h.client.isMutating() === 0);
      } finally {
        h.stop();
      }
    }
});

await test("a current successful user operation still navigates under its captured workspace", async () => {
  for (const [file, flow] of flows) {
    const h = harness(file);
    try {
      const response = h.queue();
      const done = invoke(h, file, flow);
      await until(() => h.calls.some((call) => call.method === "POST" || call.method === "PATCH"));
      response.resolve({ key: "OLD", displayId: "WIKI-7" });
      await done;
      await until(() => h.navigation.length === 1);
      const destination = h.navigation[0];
      assert.ok(destination, "the current operation must navigate");
      assert.match(destination, /^\/w\/alpha\//);
    } finally {
      h.stop();
    }
  }
});
