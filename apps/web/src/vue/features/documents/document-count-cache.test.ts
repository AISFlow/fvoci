import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

// Execute the actual SFC mutation callbacks with a deferred transport. This
// checks captured query keys and late navigation without opening collab sockets.
for (const name of ["WikiDocumentView", "ProjectDocumentView"]) {
  test(`${name}: delayed trash/move invalidate captured counts and cannot mutate a new room`, async () => {
    const source = readFileSync(new URL(`./${name}.vue`, import.meta.url), "utf8");
    const from = source.indexOf("type DocumentOperation =");
    const to = source.indexOf("const patchMeta =", from);
    assert.ok(from > 0 && to > from);
    const script = ts.transpile(source.slice(from, to), { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
    const props = { workspaceId: "old-workspace", documentId: "old-document", slug: "old" };
    const scope = { value: { workspaceId: props.workspaceId, documentId: props.documentId, projectId: name.startsWith("Project") ? "old-project" : null } };
    const session = { value: { generation: 1 } };
    const lifecycleError = { value: "previous error" }; const moveParentId = { value: "parent" };
    const invalidated: unknown[][] = []; const navigation: string[] = []; const requests: unknown[] = [];
    type Operation = { scope: typeof scope.value; slug: string; session: typeof session.value; newParentId?: string };
    type Mutation = { mutationFn: (op: Operation) => Promise<unknown>; onSuccess: (data: unknown, op: Operation) => Promise<void>; onError: (error: unknown, op: Operation) => void };
    const mutations: Mutation[] = [];
    let complete!: () => void;
    const pending = new Promise<void>((resolve) => { complete = resolve; });
    const useMutation = (options: Mutation) => {
      mutations.push(options);
      return { mutate: () => undefined };
    };
    const load = async (operation: unknown) => { requests.push(operation); await pending; return {}; };
    const setup = new Function("props", "scope", "session", "lifecycleError", "moveParentId", "useMutation", "trashDocument", "moveDocument", "queryClient", "window", "trashPath", "loadErrorMessage", `${script}\nreturn {captureOperation};`);
    const { captureOperation } = setup(props, scope, session, lifecycleError, moveParentId, useMutation, load, load,
      { invalidateQueries: async ({ queryKey }: { queryKey: unknown[] }) => { invalidated.push(queryKey); } },
      { location: { assign: (path: string) => navigation.push(path) } }, (slug: string) => `/w/${slug}/trash`, () => "failed",
    ) as { captureOperation: () => Operation };
    const old = captureOperation();
    const trash = mutations[0]!.mutationFn(old);
    const move = mutations[1]!.mutationFn({ ...old, newParentId: "parent" });
    scope.value = { ...scope.value, workspaceId: "new-workspace", documentId: "new-document" };
    props.workspaceId = "new-workspace"; props.documentId = "new-document"; props.slug = "new";
    session.value = { generation: 2 };
    complete(); await Promise.all([trash, move]);
    await mutations[0]!.onSuccess({}, old); await mutations[1]!.onSuccess({}, old);
    assert.deepEqual(requests, [old.scope, old.scope]);
    assert.equal(invalidated.filter((key) => key[0] === "projects").length, 2);
    assert.ok(invalidated.every((key) => key[1] === "old-workspace"));
    assert.deepEqual(navigation, []); assert.equal(lifecycleError.value, "previous error"); assert.equal(moveParentId.value, "parent");
    mutations[0]!.onError(new Error("late"), old); assert.equal(lifecycleError.value, "previous error");
    const current = captureOperation(); await mutations[0]!.onSuccess({}, current);
    assert.deepEqual(navigation, ["/w/new/trash"]);
    // Metadata-only mutations keep their existing keys, without a count refresh.
    const meta = source.slice(source.indexOf("const patchMeta ="), source.indexOf("const notFound ="));
    assert.equal(meta.includes('["projects"'), false);
  });
}
