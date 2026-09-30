import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { computed, effectScope, onScopeDispose, reactive, shallowRef, watch } from "vue";

// Execute the actual SFC callbacks AND useCollabRoom's computed session return.
// A handmade session ref misses the snapshot replacement caused by ordinary ACKs.
const roomSource = readFileSync(new URL("../../collab/useCollabRoom.ts", import.meta.url), "utf8");
const from = roomSource.indexOf("return computed<CollabRoomSession>");
const to = roomSource.indexOf("}));", from) + 4;
assert.ok(from > 0 && to > from);
const roomScript = ts.transpile(roomSource.slice(from, to), { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });

type Scope = { workspaceId: string; documentId: string; projectId: string | null };
type Operation = { scope: Scope; slug: string; newParentId?: string };
type Mutation = {
  mutationFn: (op: Operation) => Promise<unknown>;
  onSuccess: (data: unknown, op: Operation) => Promise<void>;
  onError: (error: unknown, op: Operation) => void;
};
function assertCapturedKeys(name: string, keys: unknown[][]) {
  const counts = [["projects", "old-workspace"], ["me", "workspaces"]];
  const documentKeys = name.startsWith("Project")
    ? [["project-documents", "old-workspace", "old-project"], ["project-document", "old-workspace", "old-project", "old-document"]]
    : [["tree", "old-workspace"], ["document", "old-workspace", "old-document"], ["ancestors", "old-workspace", "old-document"]];
  assert.deepEqual(keys.map((key) => JSON.stringify(key)).sort(), [...counts, ...counts, ...documentKeys].map((key) => JSON.stringify(key)).sort());
}
function harness(name: string) {
  const source = readFileSync(new URL(`./${name}.vue`, import.meta.url), "utf8");
  const from = source.indexOf("type DocumentOperation =");
  const to = source.indexOf("const patchMeta =", from);
  assert.ok(from > 0 && to > from);
  const script = ts.transpile(source.slice(from, to), { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
  const props = reactive({ workspaceId: "old-workspace", documentId: "old-document", slug: "old" });
  const projectId = shallowRef<string | null>(name.startsWith("Project") ? "old-project" : null);
  const scope = computed<Scope>(() => ({ workspaceId: props.workspaceId, documentId: props.documentId, projectId: projectId.value }));
  const collabUser = shallowRef({ id: "actor" });
  const peers = shallowRef<unknown[]>([]); const unsent = shallowRef(false);
  const bind = shallowRef({ ack: { saved: false } });
  function createSession(generation = 1, provider = {}, doc = {}) {
    return new Function("computed", "generation", "provider", "doc", "fragment", "unauthorized", "room", "connectionStatus", "collabStatusOf", "synced", "unsent", "readOnly", "isDurablySaved", "bind", "peers", "persist", roomScript)(
      computed, generation, provider, doc, {}, shallowRef(false), shallowRef({ refusal: null }), shallowRef("connected"), () => "connected", shallowRef(true), unsent, shallowRef(false), (ack: { saved: boolean }) => ack.saved, bind, peers, () => Promise.resolve(),
    ) as ReturnType<typeof computed<{ generation: number; provider: object; doc: object }>>;
  }
  const active = shallowRef<{ session: ReturnType<typeof createSession> } | null>({ session: createSession() });
  const session = computed(() => active.value?.session.value ?? null);
  const lifecycleError = shallowRef<string | null>("previous error"); const moveParentId = shallowRef("parent");
  const invalidated: unknown[][] = []; const navigation: string[] = []; const requests: unknown[] = []; const mutations: Mutation[] = [];
  let complete!: () => void;
  const pending = new Promise<void>((resolve) => { complete = resolve; });
  const load = async (operation: unknown) => { requests.push(operation); await pending; return {}; };
  const setup = new Function("props", "scope", "session", "collabUser", "watch", "onScopeDispose", "lifecycleError", "moveParentId", "useMutation", "trashDocument", "moveDocument", "queryClient", "window", "trashPath", "loadErrorMessage", `${script}\nreturn {captureOperation, currentOperation};`);
  const lifetime = effectScope();
  const operations = lifetime.run(() => setup(props, scope, session, collabUser, watch, onScopeDispose, lifecycleError, moveParentId,
    (options: Mutation) => { mutations.push(options); return { mutate: () => undefined }; }, load, load,
    { invalidateQueries: async ({ queryKey }: { queryKey: unknown[] }) => { invalidated.push(queryKey); } },
    { location: { assign: (path: string) => navigation.push(path) } }, (slug: string) => `/w/${slug}/trash`, () => "failed",
  )) as { captureOperation: () => Operation; currentOperation: (op: Operation) => boolean };
  return { source, props, projectId, collabUser, scope, peers, unsent, bind, active, session, createSession, lifecycleError, moveParentId, invalidated, navigation, requests, mutations, complete, lifetime, ...operations };
}

for (const name of ["WikiDocumentView", "ProjectDocumentView"]) {
  for (const update of ["peers", "pending", "ACK"] as const) {
    test(`${name}: same-room ${update} completes trash/move and displays failures`, async () => {
      const h = harness(name);
      try {
        const operation = h.captureOperation(); const before = h.session.value;
        const trash = h.mutations[0]!.mutationFn(operation);
        const move = h.mutations[1]!.mutationFn({ ...operation, newParentId: "parent" });
        if (update === "peers") h.peers.value = [{ id: "peer" }];
        if (update === "pending") h.unsent.value = true;
        if (update === "ACK") h.bind.value = { ack: { saved: true } };
        assert.notEqual(h.session.value, before, "actual computed snapshot was replaced");
        assert.equal(h.session.value?.provider, before?.provider);
        assert.equal(h.session.value?.doc, before?.doc);
        assert.equal(h.session.value?.generation, before?.generation);
        assert.equal(h.currentOperation(operation), true);
        h.complete(); await Promise.all([trash, move]);
        await h.mutations[1]!.onSuccess({}, operation);
        assert.equal(h.moveParentId.value, ""); assert.equal(h.lifecycleError.value, null);
        for (const mutation of h.mutations) {
          mutation.onError(new Error("failure"), operation);
          assert.equal(h.lifecycleError.value, "failed"); h.lifecycleError.value = "previous error";
        }
        await h.mutations[0]!.onSuccess({}, operation);
        assert.deepEqual(h.navigation, ["/w/old/trash"]); assert.equal(h.lifecycleError.value, null);
        assert.deepEqual(h.requests, [operation.scope, operation.scope]);
        assert.equal(h.invalidated.filter((key) => key[0] === "projects").length, 2);
        assert.equal(h.invalidated.filter((key) => JSON.stringify(key) === '["me","workspaces"]').length, 2);
        assertCapturedKeys(name, h.invalidated);
      } finally { h.lifetime.stop(); }
    });
  }
  for (const change of ["workspace", "document", "project", "slug", "provider", "doc", "generation", "actor", "actor roundtrip", "disposal"]) {
    test(`${name}: retired ${change} suppresses success/error but invalidates captured counts`, async () => {
      const h = harness(name);
      try {
        const old = h.captureOperation();
        const trash = h.mutations[0]!.mutationFn(old); const move = h.mutations[1]!.mutationFn({ ...old, newParentId: "parent" });
        const before = h.session.value!;
        if (change === "workspace") h.props.workspaceId = "new-workspace";
        if (change === "document") h.props.documentId = "new-document";
        if (change === "project") h.projectId.value = "new-project";
        if (change === "slug") h.props.slug = "new";
        if (change === "provider") h.active.value = { session: h.createSession(before.generation, {}, before.doc) };
        if (change === "doc") h.active.value = { session: h.createSession(before.generation, before.provider, {}) };
        if (change === "generation") h.active.value = { session: h.createSession(2, before.provider, before.doc) };
        if (change.startsWith("actor")) h.collabUser.value = { id: "new-actor" };
        if (change === "actor roundtrip") h.collabUser.value = { id: "actor" };
        if (change === "disposal") h.lifetime.stop();
        h.complete(); await Promise.all([trash, move]);
        assert.equal(h.currentOperation(old), false);
        await h.mutations[0]!.onSuccess({}, old); await h.mutations[1]!.onSuccess({}, old);
        for (const mutation of h.mutations) mutation.onError(new Error("late"), old);
        assert.deepEqual(h.requests, [old.scope, old.scope]);
        assert.equal(h.invalidated.filter((key) => key[0] === "projects").length, 2);
        assert.equal(h.invalidated.filter((key) => JSON.stringify(key) === '["me","workspaces"]').length, 2);
        assertCapturedKeys(name, h.invalidated);
        assert.deepEqual(h.navigation, []); assert.equal(h.lifecycleError.value, "previous error"); assert.equal(h.moveParentId.value, "parent");
        if (change !== "disposal") {
          await h.mutations[0]!.onSuccess({}, h.captureOperation());
          assert.deepEqual(h.navigation, [`/w/${h.props.slug}/trash`]);
        }
        const meta = h.source.slice(h.source.indexOf("const patchMeta ="), h.source.indexOf("const notFound ="));
        assert.equal(meta.includes('["projects"'), false); assert.equal(meta.includes('["me", "workspaces"]'), false);
      } finally { h.lifetime.stop(); }
    });
  }
}
