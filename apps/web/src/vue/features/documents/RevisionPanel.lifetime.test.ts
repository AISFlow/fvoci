import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import * as Query from "@tanstack/vue-query";
import ts from "typescript";
import * as Vue from "vue";
import { parse } from "vue/compiler-sfc";
import { persistThenCreate } from "../../../features/documents/revision-persist";
import { ProblemError } from "../../../lib/api";

function deferred() {
  let resolve!: (value?: unknown) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<unknown>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function record(value: unknown): Record<string, unknown> {
  assert.ok(typeof value === "object" && value !== null);
  return value as Record<string, unknown>;
}
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

// Execute the retained SFC setup with real Vue lifetime/reactivity and Vue Query
// mutations. HTTP responses and the host DOM are controlled, not Rust/DB witnesses.
function harness() {
  const ack = deferred();
  let persists = 0;
  const props = Vue.reactive({
    workspaceId: "workspace-A",
    documentId: "document-A",
    projectId: null as string | null,
    targetKind: "document",
    readOnly: false,
    persistNow: () => {
      persists++;
      return ack.promise;
    },
  });
  const actor = Vue.ref({ userId: "actor-A", sessionId: "session-A", timezone: "UTC" });
  const meError = Vue.ref<unknown>(null);
  const calls: { method: string; args: unknown[] }[] = [];
  const responses: ReturnType<typeof deferred>[] = [];
  const client = new Query.QueryClient({ defaultOptions: { mutations: { retry: false } } });
  client.invalidateQueries = (options?: Query.InvalidateQueryFilters) => {
    calls.push({ method: "invalidate", args: [...(options?.queryKey ?? [])] });
    return Promise.resolve();
  };
  const request = (method: string, args: unknown[]) => {
    calls.push({ method, args });
    return responses.shift()?.promise ?? Promise.resolve({ id: "revision-A" });
  };
  const injected = {
    ...Vue,
    defineProps: () => props,
    withDefaults: (value: unknown) => value,
    t: (key: string) => key,
    formatPersonName: () => "actor",
    useMutation: Query.useMutation,
    useQueryClient: Query.useQueryClient,
    useQuery: () => ({
      data: actor,
      error: meError,
      isLoading: Vue.ref(false),
      isError: Vue.computed(() => meError.value !== null),
    }),
    meQuery: {},
    membersQuery: () => ({}),
    persistThenCreate,
    ProblemError,
    createRevision: (...args: unknown[]) => request("create", args),
    restoreRevision: (...args: unknown[]) => request("restore", args),
    getRevision: (...args: unknown[]) => request("preview", args),
    crypto,
    document: { activeElement: null },
    HTMLElement: class {},
  };
  const { descriptor } = parse(readFileSync(new URL("RevisionPanel.vue", import.meta.url), "utf8"));
  assert.ok(descriptor.scriptSetup);
  let source = descriptor.scriptSetup.content;
  const parsed = ts.createSourceFile("panel.ts", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  for (const statement of [...parsed.statements].reverse())
    if (ts.isImportDeclaration(statement))
      source = source.slice(0, statement.getFullStart()) + source.slice(statement.end);
  let panel: Record<string, unknown>;
  let mounted = true;
  const app = renderer.createApp({
    setup() {
      const javascript = new Bun.Transpiler({ loader: "ts" }).transformSync(
        `(() => {${source}\nreturn {save,restore,showPreview,notice,preview,pendingRestoreId,
          confirmRestore: typeof confirmRestore === 'function' ? confirmRestore : () => restore.mutate(pendingRestoreId.value)};})()`,
      );
      panel = record(runInNewContext(javascript, injected));
      return () => null;
    },
  });
  app.use(Query.VueQueryPlugin, { queryClient: client });
  app.mount({});
  function call(name: string, ...args: unknown[]) {
    const fn = panel[name];
    assert.equal(typeof fn, "function");
    return (fn as (...args: unknown[]) => unknown)(...args);
  }
  function value(name: string) {
    const ref = panel[name];
    assert.ok(Vue.isRef(ref));
    return ref.value as unknown;
  }
  function set(name: string, value: unknown) {
    const ref = panel[name];
    assert.ok(Vue.isRef(ref));
    ref.value = value;
  }
  function detach() {
    if (mounted) app.unmount();
    mounted = false;
  }
  return {
    ack, props, actor, meError, calls, client, call, value, set, detach,
    persists: () => persists,
    queue() {
      const response = deferred();
      responses.push(response);
      return response;
    },
    stop() { detach(); client.clear(); },
  };
}
async function settle() {
  for (let step = 0; step < 10; step++) {
    await Vue.nextTick();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

await test("delayed persist ACK: repeated save starts exactly one persist and revision", async () => {
  const h = harness();
  try {
    h.call("save");
    h.call("save");
    h.ack.resolve();
    await settle();
    assert.equal(h.persists(), 1, "single flight includes the persist wait");
    assert.equal(h.calls.filter((call) => call.method === "create").length, 1);
  } finally { h.stop(); }
});

function transition(h: ReturnType<typeof harness>, change: string) {
  if (change === "aba") {
    h.props.documentId = "document-B";
    h.props.documentId = "document-A";
  }
  if (change === "workspace") h.props.workspaceId = "workspace-B";
  if (change === "project") h.props.projectId = "project-B";
  if (change === "kind") h.props.targetKind = "task";
  if (change === "actor") h.actor.value = { ...h.actor.value, userId: "actor-B" };
  if (change === "actor-aba") {
    h.actor.value = { ...h.actor.value, userId: "actor-B" };
    h.actor.value = { ...h.actor.value, userId: "actor-A" };
  }
  if (change === "session") h.actor.value = { ...h.actor.value, sessionId: "session-B" };
  if (change === "signed-out") h.meError.value = new ProblemError(401);
  if (change === "readonly") h.props.readOnly = true;
  if (change === "dispose") h.detach();
}
const retirements = ["aba", "workspace", "project", "kind", "actor", "actor-aba", "session", "signed-out", "readonly", "dispose"];

await test("scope retirement including same-tick ABA prevents a late ACK from creating a revision", async () => {
  for (const change of retirements) {
    const h = harness();
    try {
      h.call("save");
      transition(h, change);
      h.ack.resolve();
      await settle();
      assert.equal(h.calls.filter((call) => call.method === "create").length, 0, change);
      assert.equal(h.value("notice"), null, change);
    } finally { h.stop(); }
  }
});

await test("old create success/error never updates notice or invalidates a new scope", async () => {
  for (const failure of [false, true]) for (const change of retirements) {
    const h = harness();
    try {
      const http = h.queue();
      h.call("save");
      h.ack.resolve();
      await settle();
      assert.equal(h.calls.filter((call) => call.method === "create").length, 1);
      transition(h, change);
      if (failure) http.reject(new Error("old create failed"));
      else http.resolve();
      await settle();
      assert.equal(h.calls.filter((call) => call.method === "invalidate").length, 0, change);
      assert.equal(h.value("notice"), null, change);
    } finally { h.stop(); }
  }
});

await test("old restore success/timeout never updates a new scope or its document cache", async () => {
  for (const failure of [false, true]) for (const change of retirements) {
    const h = harness();
    try {
      const http = h.queue();
      h.set("pendingRestoreId", "revision-old");
      h.call("confirmRestore");
      await settle();
      assert.equal(h.calls.filter((call) => call.method === "restore").length, 1);
      transition(h, change);
      if (failure) http.reject(new ProblemError(504));
      else http.resolve();
      await settle();
      assert.equal(h.calls.filter((call) => call.method === "invalidate").length, 0, change);
      assert.equal(h.value("notice"), null, change);
    } finally { h.stop(); }
  }
});

await test("preview latest selection wins and retirement discards old success/error", async () => {
  const h = harness();
  try {
    const first = h.queue();
    const old = h.call("showPreview", "first");
    const second = h.queue();
    const latest = h.call("showPreview", "second");
    second.resolve({ id: "second", contentJson: {} });
    await latest;
    first.resolve({ id: "first", contentJson: {} });
    await old;
    assert.equal(record(h.value("preview")).id, "second");
  } finally { h.stop(); }
  for (const failure of [false, true]) for (const change of retirements) {
    const h = harness();
    try {
      const http = h.queue();
      const done = h.call("showPreview", "old");
      transition(h, change);
      if (failure) http.reject(new Error("old preview failed"));
      else http.resolve({ id: "old", contentJson: {} });
      await done;
      assert.equal(h.value("notice"), null, change);
      assert.equal(h.value("preview"), null, change);
    } finally { h.stop(); }
  }
});

await test("delayed persist ACK: changing the target retires the pending save", async () => {
  const h = harness();
  try {
    h.call("save");
    h.props.documentId = "document-B";
    h.ack.resolve();
    await settle();
    assert.equal(h.calls.filter((call) => call.method === "create").length, 0);
    assert.equal(h.value("notice"), null);
  } finally { h.stop(); }
});
