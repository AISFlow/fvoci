import { evaluateTestFunction } from "../share/evaluate-test-function.test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { execFileSync } from "node:child_process";
import ts from "typescript";
import { QueryClient } from "@tanstack/vue-query";
import { computed, effectScope, onScopeDispose, reactive, shallowRef, watch } from "vue";

// Execute the actual SFC callbacks AND useCollabRoom's computed session return.
// A handmade session ref misses the snapshot replacement caused by ordinary ACKs.
const roomSource = readFileSync(new URL("../../collab/useCollabRoom.ts", import.meta.url), "utf8");
const from = roomSource.indexOf("return computed<CollabRoomSession>");
const to = roomSource.indexOf("}));", from) + 4;
assert.ok(from > 0 && to > from);
const roomScript = ts.transpile(roomSource.slice(from, to), {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.None,
});

const roomFactory = await evaluateTestFunction(
  [
    "computed",
    "generation",
    "provider",
    "doc",
    "fragment",
    "unauthorized",
    "room",
    "connectionStatus",
    "collabStatusOf",
    "synced",
    "unsent",
    "readOnly",
    "isDurablySaved",
    "bind",
    "peers",
    "persist",
  ],
  roomScript,
);

type Scope = { workspaceId: string; documentId: string; projectId: string | null };
type Operation = { scope: Scope; slug: string; newParentId?: string };
type Mutation = {
  mutationFn: (op: Operation) => Promise<unknown>;
  onSuccess: (data: unknown, op: Operation) => Promise<void>;
  onError: (error: unknown, op: Operation) => void;
};
function assertCapturedKeys(name: string, keys: unknown[][]) {
  const counts = [
    ["projects", "old-workspace"],
    ["wiki-discovery", "old-workspace"],
    ["me", "workspaces"],
  ];
  const documentKeys = name.startsWith("Project")
    ? [
        ["project-documents", "old-workspace", "old-project"],
        ["project-document", "old-workspace", "old-project", "old-document"],
      ]
    : [
        ["tree", "old-workspace"],
        ["document", "old-workspace", "old-document"],
        ["ancestors", "old-workspace", "old-document"],
      ];
  assert.deepEqual(
    keys.map((key) => JSON.stringify(key)).sort(),
    [...counts, ...counts, ...documentKeys, ["trash", "old-workspace"]]
      .map((key) => JSON.stringify(key))
      .sort(),
  );
}
async function harness(name: string, includePatch = false) {
  const source = componentSource(`documents/${name}.vue`);
  const from = source.indexOf("type DocumentOperation =");
  const to = source.indexOf(includePatch ? "const notFound =" : "const patchMeta =", from);
  assert.ok(from > 0 && to > from);
  const script = ts.transpile(source.slice(from, to), {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.None,
  });
  const props = reactive({ workspaceId: "old-workspace", documentId: "old-document", slug: "old" });
  const projectId = shallowRef<string | null>(name.startsWith("Project") ? "old-project" : null);
  const scope = computed<Scope>(() => ({
    workspaceId: props.workspaceId,
    documentId: props.documentId,
    projectId: projectId.value,
  }));
  const collabUser = shallowRef({ id: "actor" });
  const peers = shallowRef<unknown[]>([]);
  const unsent = shallowRef(false);
  const bind = shallowRef({ ack: { saved: false } });
  function createSession(generation = 1, provider = {}, doc = {}) {
    return roomFactory(
      computed,
      generation,
      provider,
      doc,
      {},
      shallowRef(false),
      shallowRef({ refusal: null }),
      shallowRef("connected"),
      () => "connected",
      shallowRef(true),
      unsent,
      shallowRef(false),
      (ack: { saved: boolean }) => ack.saved,
      bind,
      peers,
      () => Promise.resolve(),
    ) as ReturnType<typeof computed<{ generation: number; provider: object; doc: object }>>;
  }
  const active = shallowRef<{ session: ReturnType<typeof createSession> } | null>({
    session: createSession(),
  });
  const session = computed(() => active.value?.session.value ?? null);
  const saveError = shallowRef<string | null>("previous save error");
  const lifecycleError = shallowRef<string | null>("previous error");
  const moveParentId = shallowRef("parent");
  const invalidated: unknown[][] = [];
  const navigation: string[] = [];
  const requests: unknown[] = [];
  const mutations: Mutation[] = [];
  let complete!: () => void;
  const pending = new Promise<void>((resolve) => {
    complete = resolve;
  });
  const load = async (operation: unknown) => {
    requests.push(operation);
    await pending;
    return {};
  };
  const queryClient = new QueryClient();
  for (const workspaceId of ["old-workspace", "new-workspace"]) {
    for (const tag of ["", "tag"])
      queryClient.setQueryData(["wiki-discovery", workspaceId, tag], { totalCount: 2 });
  }
  const setup = await evaluateTestFunction(
    [
      "props",
      "scope",
      "session",
      "bodyDoc",
      "bodyGeneration",
      "collabUser",
      "watch",
      "onScopeDispose",
      "lifecycleError",
      "moveParentId",
      "useMutation",
      "trashDocument",
      "moveDocument",
      "queryClient",
      "router",
      "trashPath",
      "loadErrorMessage",
      "patchDocument",
      "saveError",
      "metaKey",
      "treeKey",
    ],
    `${script}\nreturn {captureOperation, currentOperation};`,
  );
  const lifetime = effectScope();
  const operations = lifetime.run(() =>
    setup(
      props,
      scope,
      session,
      computed(() => session.value?.doc),
      computed(() => session.value?.generation),
      collabUser,
      watch,
      onScopeDispose,
      lifecycleError,
      moveParentId,
      (options: Mutation) => {
        mutations.push(options);
        return { mutate: () => undefined };
      },
      load,
      load,
      {
        invalidateQueries: async ({ queryKey }: { queryKey: unknown[] }) => {
          invalidated.push(queryKey);
          await queryClient.invalidateQueries({ queryKey });
        },
      },
      {
        push: (path: string) => {
          navigation.push(path);
          return Promise.resolve();
        },
      },
      (slug: string) => `/w/${slug}/trash`,
      () => "failed",
      load,
      saveError,
      computed(() => ["document", props.workspaceId, props.documentId]),
      computed(() => ["tree", props.workspaceId]),
    ),
  ) as { captureOperation: () => Operation; currentOperation: (op: Operation) => boolean };
  return {
    saveError,
    source,
    props,
    projectId,
    collabUser,
    queryClient,
    scope,
    peers,
    unsent,
    bind,
    active,
    session,
    createSession,
    lifecycleError,
    moveParentId,
    invalidated,
    navigation,
    requests,
    mutations,
    complete,
    lifetime,
    ...operations,
  };
}

for (const name of ["WikiDocumentView", "ProjectDocumentView"]) {
  for (const update of ["peers", "pending", "ACK"] as const) {
    await test(`${name}: same-room ${update} completes trash/move and displays failures`, async () => {
      const h = await harness(name);
      try {
        const operation = h.captureOperation();
        const before = h.session.value;
        const trash = required(h.mutations[0]).mutationFn(operation);
        const move = required(h.mutations[1]).mutationFn({ ...operation, newParentId: "parent" });
        if (update === "peers") h.peers.value = [{ id: "peer" }];
        if (update === "pending") h.unsent.value = true;
        if (update === "ACK") h.bind.value = { ack: { saved: true } };
        assert.notEqual(h.session.value, before, "actual computed snapshot was replaced");
        assert.equal(h.session.value?.provider, before?.provider);
        assert.equal(h.session.value?.doc, before?.doc);
        assert.equal(h.session.value?.generation, before?.generation);
        assert.equal(h.currentOperation(operation), true);
        h.complete();
        await Promise.all([trash, move]);
        await required(h.mutations[1]).onSuccess({}, operation);
        assert.equal(h.moveParentId.value, "");
        assert.equal(h.lifecycleError.value, null);
        for (const mutation of h.mutations) {
          mutation.onError(new Error("failure"), operation);
          assert.equal(h.lifecycleError.value, "failed");
          h.lifecycleError.value = "previous error";
        }
        await required(h.mutations[0]).onSuccess({}, operation);
        assert.deepEqual(h.navigation, ["/w/old/trash"]);
        assert.equal(h.lifecycleError.value, null);
        assert.deepEqual(h.requests, [operation.scope, operation.scope]);
        assert.equal(h.invalidated.filter((key) => key[0] === "projects").length, 2);
        assert.equal(
          h.invalidated.filter((key) => JSON.stringify(key) === '["me","workspaces"]').length,
          2,
        );
        assertCapturedKeys(name, h.invalidated);
        for (const tag of ["", "tag"]) {
          assert.equal(
            h.queryClient.getQueryState(["wiki-discovery", "old-workspace", tag])?.isInvalidated,
            true,
          );
          assert.equal(
            h.queryClient.getQueryState(["wiki-discovery", "new-workspace", tag])?.isInvalidated,
            false,
          );
        }
      } finally {
        h.lifetime.stop();
        h.queryClient.clear();
      }
    });
  }
  for (const change of [
    "workspace",
    "document",
    "project",
    "slug",
    "provider",
    "doc",
    "generation",
    "actor",
    "actor roundtrip",
    "disposal",
  ]) {
    await test(`${name}: retired ${change} suppresses success/error but invalidates captured counts`, async () => {
      const h = await harness(name);
      try {
        const old = h.captureOperation();
        const trash = required(h.mutations[0]).mutationFn(old);
        const move = required(h.mutations[1]).mutationFn({ ...old, newParentId: "parent" });
        const before = required(h.session.value);
        if (change === "workspace") h.props.workspaceId = "new-workspace";
        if (change === "document") h.props.documentId = "new-document";
        if (change === "project") h.projectId.value = "new-project";
        if (change === "slug") h.props.slug = "new";
        if (change === "provider")
          h.active.value = { session: h.createSession(before.generation, {}, before.doc) };
        if (change === "doc")
          h.active.value = { session: h.createSession(before.generation, before.provider, {}) };
        if (change === "generation")
          h.active.value = { session: h.createSession(2, before.provider, before.doc) };
        if (change.startsWith("actor")) h.collabUser.value = { id: "new-actor" };
        if (change === "actor roundtrip") h.collabUser.value = { id: "actor" };
        if (change === "disposal") h.lifetime.stop();
        h.complete();
        await Promise.all([trash, move]);
        assert.equal(h.currentOperation(old), false);
        await required(h.mutations[0]).onSuccess({}, old);
        await required(h.mutations[1]).onSuccess({}, old);
        for (const mutation of h.mutations) mutation.onError(new Error("late"), old);
        assert.deepEqual(h.requests, [old.scope, old.scope]);
        assert.equal(h.invalidated.filter((key) => key[0] === "projects").length, 2);
        assert.equal(
          h.invalidated.filter((key) => JSON.stringify(key) === '["me","workspaces"]').length,
          2,
        );
        assertCapturedKeys(name, h.invalidated);
        for (const tag of ["", "tag"]) {
          assert.equal(
            h.queryClient.getQueryState(["wiki-discovery", "old-workspace", tag])?.isInvalidated,
            true,
          );
          assert.equal(
            h.queryClient.getQueryState(["wiki-discovery", "new-workspace", tag])?.isInvalidated,
            false,
          );
        }
        assert.deepEqual(h.navigation, []);
        assert.equal(h.lifecycleError.value, "previous error");
        assert.equal(h.moveParentId.value, "parent");
        if (change !== "disposal") {
          await required(h.mutations[0]).onSuccess({}, h.captureOperation());
          assert.deepEqual(h.navigation, [`/w/${h.props.slug}/trash`]);
        }
        const meta = h.source.slice(
          h.source.indexOf("const patchMeta ="),
          h.source.indexOf("const notFound ="),
        );
        assert.equal(meta.includes('["projects"'), false);
        assert.equal(meta.includes('["me", "workspaces"]'), false);
        assert.equal(meta.includes('["wiki-discovery"'), true);
      } finally {
        h.lifetime.stop();
        h.queryClient.clear();
      }
    });
  }
}

function componentSource(path: string): string {
  if (process.env.FVOCI_CACHE_BASELINE)
    return execFileSync(
      "git",
      ["show", `${process.env.FVOCI_CACHE_BASELINE}:apps/web/src/vue/features/${path}`],
      { encoding: "utf8" },
    );
  return readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
}
for (const name of ["WikiDocumentView", "ProjectDocumentView"]) {
  for (const retired of [false, true])
    await test(`${name}: metadata refreshes fresh discovery without counts (retired=${String(retired)})`, async () => {
      const h = await harness(name, true);
      try {
        const op = { ...h.captureOperation(), body: { title: "renamed" } };
        const callback = required(h.mutations[2]);
        const request = callback.mutationFn(op);
        if (retired) {
          h.props.workspaceId = "new-workspace";
          h.collabUser.value = { id: "other" };
        }
        h.complete();
        await request;
        assert.deepEqual(h.requests[0], op.scope);
        await callback.onSuccess({}, op);
        for (const tag of ["", "tag"]) {
          let requests = 0;
          const result = await h.queryClient.query({
            queryKey: ["wiki-discovery", "old-workspace", tag],
            staleTime: 30_000,
            queryFn: () => {
              requests++;
              return Promise.resolve({ title: "renamed", totalCount: 2 });
            },
          });
          assert.equal(
            requests,
            1,
            "successful mutation must retire the still-fresh discovery cache",
          );
          assert.equal(result.title, "renamed");
          assert.equal(
            h.queryClient.getQueryState(["wiki-discovery", "new-workspace", tag])?.isInvalidated,
            false,
          );
        }
        assert.equal(
          h.invalidated.some((key) => key[0] === "projects" || key[0] === "me"),
          false,
        );
        assert.ok(h.invalidated.every((key) => key[1] === "old-workspace"));
        assert.equal(h.saveError.value, retired ? "previous save error" : null);
        callback.onError(new Error("late"), op);
        assert.equal(h.saveError.value, retired ? "previous save error" : "failed");
      } finally {
        h.lifetime.stop();
        h.queryClient.clear();
      }
    });
}

type LocalOperation = {
  workspaceId: string;
  documentId?: string;
  projectId: string | null;
  tagId?: string;
  name?: string;
  lifecycle?: number;
};
type LocalMutation = {
  mutationFn: (op: LocalOperation) => Promise<unknown>;
  onSuccess: (data: unknown, op: LocalOperation) => Promise<void>;
  onError: (err: unknown, op: LocalOperation) => void;
};
async function localHarness(kind: "tags" | "restore") {
  const source = componentSource(
    kind === "tags" ? "documents/DocumentTagsBar.vue" : "settings/DeletedProjectsSection.vue",
  );
  const start = source.indexOf(kind === "tags" ? "type TagOperation" : "type RestoreOperation");
  // Baseline uses live props; execute its real callback for the negative cache proof.
  const baselineStart = source.indexOf(
    kind === "tags" ? "async function invalidate()" : "const restore =",
  );
  const end = source.indexOf(kind === "tags" ? "const pending =" : "const items =");
  const script = ts.transpile(source.slice(start >= 0 ? start : baselineStart, end), {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.None,
  });
  const props = reactive({
    workspaceId: "old-workspace",
    documentId: "old-document",
    projectId: null as string | null,
    readOnly: false,
  });
  const me = { data: shallowRef({ userId: "actor", sessionId: "session" }) };
  const error = shallowRef<string | null>("previous error");
  const restoreTarget = shallowRef("project");
  const open = shallowRef(true);
  const filter = shallowRef("tag");
  const mutations: LocalMutation[] = [];
  const requests: unknown[] = [];
  const client = new QueryClient();
  for (const ws of ["old-workspace", "new-workspace"]) {
    for (const tag of ["", "tag"]) client.setQueryData(["wiki-discovery", ws, tag], { items: [] });
    for (const prefix of ["projects", "trash", "document-tags"])
      client.setQueryData([prefix, ws], {});
    client.setQueryData(
      ["document-tags", ws, "assigned", `${ws === "old-workspace" ? "old" : "new"}-document`],
      [],
    );
  }
  client.setQueryData(["me", "workspaces"], { items: [] });
  const assignedQuery = (ws: string, doc: string) => ({
    queryKey: ["document-tags", ws, "assigned", doc],
  });
  const load = (...args: unknown[]) => {
    requests.push(args);
    return Promise.resolve({});
  };
  const life = effectScope();
  const setup = await evaluateTestFunction(
    [
      "props",
      "me",
      "watch",
      "onScopeDispose",
      "queryClient",
      "client",
      "error",
      "mutationError",
      "restoreTarget",
      "open",
      "filter",
      "trigger",
      "HTMLElement",
      "useMutation",
      "documentAssignedTagsQuery",
      "assignedOptions",
      "assignDocumentTag",
      "createDocumentTag",
      "removeDocumentTag",
      "api",
      "ensureOk",
      "loadErrorMessage",
      "problemMessage",
    ],
    `${script}\nreturn typeof captureOperation === 'function' ? captureOperation : (projectId) => ({workspaceId: props.workspaceId, documentId: props.documentId, projectId});`,
  );
  const capture = life.run(() =>
    setup(
      props,
      me,
      watch,
      onScopeDispose,
      client,
      client,
      error,
      error,
      restoreTarget,
      open,
      filter,
      shallowRef(null),
      class {
        marker = "fixture-element";
      },
      (options: LocalMutation) => {
        mutations.push(options);
        return {
          mutate: () => {},
          mutateAsync: async (op: LocalOperation) => {
            const result = await options.mutationFn(op);
            await options.onSuccess({ id: "tag", name: "tag" }, op);
            return result;
          },
        };
      },
      assignedQuery,
      computed(() => assignedQuery(props.workspaceId, props.documentId)),
      load,
      load,
      load,
      { POST: load },
      (value: unknown) => value,
      () => "failed",
      () => "failed",
    ),
  ) as (projectId?: string) => LocalOperation;
  return {
    client,
    props,
    me,
    error,
    restoreTarget,
    open,
    filter,
    mutations,
    requests,
    life,
    capture,
  };
}
for (const kind of ["tags", "restore"] as const) {
  for (const retired of [false, true])
    await test(`${kind}: captured success refreshes fresh discovery and suppresses retired UI (${String(retired)})`, async () => {
      const h = await localHarness(kind);
      try {
        const op = { ...h.capture("project"), tagId: "tag", name: "new tag" };
        if (retired) {
          h.props.workspaceId = "new-workspace";
          h.props.documentId = "new-document";
          h.me.data.value = { userId: "other", sessionId: "new" };
        }
        await required(h.mutations[0]).onSuccess({ id: "tag", name: "tag" }, op);
        for (const tag of ["", "tag"]) {
          let requests = 0;
          await h.client.query({
            queryKey: ["wiki-discovery", "old-workspace", tag],
            staleTime: 30_000,
            queryFn: () => {
              requests++;
              return { items: ["updated"] };
            },
          });
          assert.equal(requests, 1, "mutation must refresh the cached original list before 30s");
          assert.equal(
            h.client.getQueryState(["wiki-discovery", "new-workspace", tag])?.isInvalidated,
            false,
          );
        }
        assert.equal(
          h.client.getQueryState(["projects", "old-workspace"])?.isInvalidated,
          kind === "restore",
        );
        assert.equal(
          h.client.getQueryState(["me", "workspaces"])?.isInvalidated,
          kind === "restore",
        );
        assert.equal(h.client.getQueryState(["projects", "new-workspace"])?.isInvalidated, false);
        if (kind === "tags") {
          assert.deepEqual(
            h.client.getQueryData(["document-tags", "old-workspace", "assigned", "old-document"]),
            [{ id: "tag", name: "tag" }],
          );
          assert.deepEqual(
            h.client.getQueryData(["document-tags", "new-workspace", "assigned", "new-document"]),
            [],
          );
          assert.equal(h.open.value, retired);
          await required(h.mutations[1]).onSuccess({ id: "tag" }, op);
          assert.deepEqual(
            h.requests[0],
            ["old-workspace", "old-document", null, "tag"],
            "create-to-assign retains the captured document",
          );
          await required(h.mutations[2]).onSuccess({}, op);
          assert.deepEqual(
            h.client.getQueryData(["document-tags", "old-workspace", "assigned", "old-document"]),
            [],
          );
          assert.equal(
            h.client.getQueryState(["wiki-discovery", "old-workspace", "tag"])?.isInvalidated,
            true,
          );
        } else {
          assert.equal(h.client.getQueryState(["trash", "old-workspace"])?.isInvalidated, true);
          assert.equal(h.restoreTarget.value, retired ? "project" : null);
        }
        required(h.mutations[0]).onError(new Error("late"), op);
        assert.equal(h.error.value, retired ? "previous error" : "failed");
      } finally {
        h.life.stop();
        h.client.clear();
      }
    });
}

for (const kind of ["tags", "restore"] as const) {
  for (const change of [
    "document",
    "project",
    "actor roundtrip",
    "session",
    "readonly",
    "disposal",
  ] as const) {
    if (kind === "restore" && ["document", "project", "readonly"].includes(change)) continue;
    await test(`${kind}: retired ${change} retains original cache target and UI`, async () => {
      const h = await localHarness(kind);
      try {
        const op = { ...h.capture("project"), tagId: "tag" };
        if (change === "document") h.props.documentId = "new-document";
        if (change === "project") h.props.projectId = "new-project";
        if (change === "actor roundtrip") {
          h.me.data.value = { userId: "other", sessionId: "session" };
          h.me.data.value = { userId: "actor", sessionId: "session" };
        }
        if (change === "session") h.me.data.value = { userId: "actor", sessionId: "new-session" };
        if (change === "readonly") h.props.readOnly = true;
        if (change === "disposal") h.life.stop();
        await required(h.mutations[0]).mutationFn(op);
        await required(h.mutations[0]).onSuccess({ id: "tag" }, op);
        required(h.mutations[0]).onError(new Error("late"), op);
        assert.equal(h.error.value, "previous error");
        assert.equal(
          h.client.getQueryState(["wiki-discovery", "old-workspace", "tag"])?.isInvalidated,
          true,
        );
        assert.equal(h.open.value, true);
        assert.equal(h.restoreTarget.value, "project");
        if (kind === "tags")
          assert.deepEqual(h.requests[0], ["old-workspace", "old-document", null, "tag"]);
        else
          assert.deepEqual(h.requests[0], [
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/restore",
            { params: { path: { workspace_id: "old-workspace", project_id: "project" } } },
          ]);
      } finally {
        h.life.stop();
        h.client.clear();
      }
    });
  }
}

function required<T>(value: T | null | undefined): T {
  assert.ok(value !== null && value !== undefined, "required fixture value");
  return value;
}
