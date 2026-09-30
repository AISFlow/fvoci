import assert from "node:assert/strict";
import test from "node:test";
import type { components } from "@/generated/api";
import { api, ProblemError } from "@/lib/api";
import { createWorkspaceEditorEntities, editorEntityTransport, type EditorEntityTransport } from "./editor-entities";

type Schema = components["schemas"];
const ws = "11111111-1111-4111-8111-111111111111";
const docId = "22222222-2222-4222-8222-222222222222";
const taskId = "33333333-3333-4333-8333-333333333333";
const userId = "44444444-4444-4444-8444-444444444444";
const groupId = "55555555-5555-4555-8555-555555555555";
const projectId = "66666666-6666-4666-8666-666666666666";
const member = { userId, givenName: "Alice", familyName: "Kim", role: "member", email: "alice@example.com" } satisfies Schema["MemberResponse"];
const group = { id: groupId, name: "Alice Team", workspaceId: ws, createdAt: "", updatedAt: "" } satisfies Schema["GroupOutput"];
const doc = {
  id: docId, title: "Real document", icon: "📄", workspaceId: ws, projectId,
  number: 1, parentId: null, path: "", status: "draft", schemaVersion: 2,
  sortKey: "", version: 1, createdAt: "", updatedAt: "", createdBy: userId,
} satisfies Schema["DocumentMetaResponse"];
const task = {
  id: taskId, title: "Real task", workspaceId: ws, projectId, statusId: "status", number: 1,
  archivedAt: null, createdAt: "", createdBy: userId, dueAt: null, dueDate: null,
  estimate: null, milestoneId: null, parentId: null, priority: "none", recurrence: null,
  schemaVersion: 2, sortKey: "", startDate: null, type: "task", updatedAt: "", version: 1,
  assigneeIds: [], canEdit: true, childProgress: null, children: [], contentJson: null,
  dependencies: [], labelIds: [], parent: null,
} satisfies Schema["TaskOutput"];
const project = {
  id: projectId, key: "PRJ", name: "Real project", icon: "🪴", status: "archived",
  description: "", visibility: "workspace", taskCount: 1, openTaskCount: 1,
  canEdit: false, canManage: false, rootDocumentId: docId, createdAt: "", updatedAt: "",
} satisfies Schema["ProjectListItemOutput"];
const lookupItems = [
  { id: taskId, kind: "task", displayId: "PRJ-1", title: "task hit", projectId },
  { id: docId, kind: "document", displayId: "PRJ-1", title: "doc hit", projectId },
] satisfies Schema["LookupItemOutput"][];
function searchItem(type: string, id: string, displayId: string | null): Schema["SearchItemOutput"] {
  return { type, id, displayId, title: `found ${type}`, workspaceId: ws, projectId,
    chunkNo: null, documentId: null, extractStatus: null, score: 1, snippet: null,
    taskId: null, updatedAt: "" };
}
function transport(overrides: Partial<EditorEntityTransport> = {}): EditorEntityTransport {
  return {
    members: async () => ({ items: [member] }), groups: async () => ({ items: [group] }),
    lookup: async () => ({ items: lookupItems }),
    search: async (_w, _q, kind) => ({ items: [searchItem(kind, kind === "task" ? taskId : docId, "PRJ-1")], nextCursor: null }),
    projects: async () => ({ items: [project] }), task: async () => task,
    documentUuid: async () => doc, document: async () => doc,
    workflow: async () => ({ id: "wf", projectId, statuses: [{ id: "status", name: "Doing", category: "active", sortKey: "", wipLimit: null, workflowId: "wf" }] }),
    ...overrides,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

test("all four menu types preserve task/document/user/group ordering and casefolded person filtering", async () => {
  const scope = createWorkspaceEditorEntities(ws, transport());
  assert.deepEqual((await scope.mentionItems("  aLiCe ")).map((x) => x.entity), ["task", "document", "user", "group"]);
  assert.deepEqual((await scope.mentionItems("someone else")).map((x) => x.entity), ["task", "document"]);
  scope.dispose();
});

test("blank loads only people; display ID loads canonical lookup rather than search", async () => {
  const queries: string[] = [];
  const scope = createWorkspaceEditorEntities(ws, transport({
    search: async () => { throw new Error("search must not run"); },
    lookup: async (_w, q) => { queries.push(q); return { items: lookupItems }; },
  }));
  assert.deepEqual((await scope.mentionItems("   ")).map((x) => x.entity), ["user", "group"]);
  assert.deepEqual((await scope.mentionItems("prj-0001")).map((x) => x.entity), ["task", "document"]);
  assert.deepEqual(queries, ["PRJ-1"]);
  scope.dispose();
});

test("member denial and a failed search retain independent authorized hits, never attachments or null IDs", async () => {
  const scope = createWorkspaceEditorEntities(ws, transport({
    members: async () => { throw new ProblemError(403); },
    search: async (_w, _q, kind) => ({ items: kind === "task" ? [] : [
      searchItem("document", docId, "WIKI-1"), searchItem("document", docId, null),
      searchItem("attachment", docId, "PRJ-1"), searchItem("comment", docId, "PRJ-1"),
      searchItem("task", "", "PRJ-1"),
    ], nextCursor: null }),
  }));
  assert.deepEqual((await scope.mentionItems("alice")).map((x) => x.entity), ["document", "group"]);
  scope.dispose();
});

test("empty and unknown lookup kinds never expand the menu", async () => {
  const scope = createWorkspaceEditorEntities(ws, transport({ lookup: async () => ({ items: [
    { ...lookupItems[0]!, kind: "project" }, { ...lookupItems[0]!, id: "" },
  ] }) }));
  assert.deepEqual(await scope.mentionItems("PRJ-1"), []);
  scope.dispose();
});

test("UUID and display resolvers use true title/icon/status, requested kind and project affiliation", async () => {
  const calls: string[] = [];
  const scope = createWorkspaceEditorEntities(ws, transport({
    document: async (_w, id, p) => { calls.push(`document:${id}:${p}`); return doc; },
    task: async (_w, id) => { calls.push(`task:${id}`); return task; },
  }));
  assert.deepEqual(await scope.entityResolver("user", userId), { label: "KimAlice", icon: "" });
  assert.deepEqual(await scope.entityResolver("group", groupId), { label: "Alice Team", icon: "" });
  assert.deepEqual(await scope.entityResolver("document", docId), { label: "Real document", icon: "📄" });
  assert.deepEqual(await scope.entityResolver("document", "PRJ-1"), { label: "Real document", icon: "📄" });
  assert.deepEqual(await scope.entityResolver("task", "PRJ-1"), { label: "Real task", icon: "", status: "Doing" });
  const byKey = await scope.entityResolver("project", "prj");
  assert.equal(byKey?.label, "Real project"); assert.equal(byKey?.icon, "🪴"); assert.ok(byKey?.status);
  assert.deepEqual(await scope.entityResolver("project", projectId), byKey);
  assert.deepEqual(calls, [`document:${docId}:${projectId}`, `task:${taskId}`]);
  scope.dispose();
});

test("wiki display references use wiki metadata and task status failure retains task card", async () => {
  let affiliation: string | null | undefined;
  const scope = createWorkspaceEditorEntities(ws, transport({
    lookup: async () => ({ items: [{ ...lookupItems[1]!, projectId: null }] }),
    document: async (_w, _id, p) => { affiliation = p; return { ...doc, projectId: null }; },
    workflow: async () => { throw new ProblemError(503); },
  }));
  assert.ok(await scope.entityResolver("document", "WIKI-0"));
  assert.equal(affiliation, null);
  assert.deepEqual(await scope.entityResolver("task", taskId), { label: "Real task", icon: "" });
  assert.equal(await scope.entityResolver("task", "WIKI-0"), null);
  scope.dispose();
});

test("foreign-workspace UUIDs, blank metadata, absent and invalid refs are inaccessible", async () => {
  let count = 0;
  const scope = createWorkspaceEditorEntities(ws, transport({
    documentUuid: async () => { count++; return { ...doc, workspaceId: projectId }; },
    task: async () => ({ ...task, title: "  " }),
  }));
  assert.equal(await scope.entityResolver("document", docId), null);
  assert.equal(await scope.entityResolver("task", taskId), null);
  for (const kind of ["user", "group", "document", "task"] as const) {
    assert.equal(await scope.entityResolver(kind, ""), null);
    assert.equal(await scope.entityResolver(kind, "bad ref"), null);
  }
  assert.equal(count, 1);
  scope.dispose();
});

test("a later permission denial is revalidated without any positive title cache", async () => {
  let permitted = true; let count = 0;
  const scope = createWorkspaceEditorEntities(ws, transport({ documentUuid: async () => {
    count++; if (!permitted) throw new ProblemError(404); return doc;
  } }));
  assert.ok(await scope.entityResolver("document", docId));
  permitted = false;
  assert.equal(await scope.entityResolver("document", docId), null);
  assert.equal(count, 2);
  scope.dispose();
});

test("slow A / fast B typeahead aborts real A signal and drops A even if transport ignores abort", async () => {
  const a = deferred<Schema["SearchListResponse"]>();
  const signals: AbortSignal[] = [];
  const scope = createWorkspaceEditorEntities(ws, transport({ search: async (_w, q, kind, signal) => {
    if (q === "old") { signals.push(signal); return a.promise; }
    return { items: [searchItem(kind, docId, "WIKI-1")], nextCursor: null };
  } }));
  const old = scope.mentionItems("old");
  assert.equal(scope.mentionItems(" old "), old, "same pending query dedupes");
  const fresh = await scope.mentionItems("new");
  assert.equal(fresh.length, 2);
  assert.ok(signals.every((s) => s instanceof AbortSignal && s.aborted));
  a.resolve({ items: [searchItem("task", taskId, "PRJ-9")], nextCursor: null });
  assert.deepEqual(await old, []);
  scope.dispose();
});

test("different nodes finish independently; same entity and shared display lookup dedupe only inflight", async () => {
  const pending = deferred<Schema["LookupListResponse"]>(); let lookups = 0;
  const scope = createWorkspaceEditorEntities(ws, transport({ lookup: async () => { lookups++; return pending.promise; } }));
  const document = scope.entityResolver("document", "PRJ-1");
  const task = scope.entityResolver("task", "PRJ-1");
  assert.equal(scope.entityResolver("document", "PRJ-1"), document);
  pending.resolve({ items: lookupItems });
  assert.ok(await document); assert.ok(await task); assert.equal(lookups, 1);
  await scope.entityResolver("document", "PRJ-1"); assert.equal(lookups, 2);
  scope.dispose();
});

test("dispose cancels all requests and rejects late node/mention/paste enrichment", async () => {
  const pending = deferred<Schema["DocumentMetaResponse"]>(); const signals: AbortSignal[] = [];
  const scope = createWorkspaceEditorEntities(ws, transport({ documentUuid: async (_id, signal) => {
    signals.push(signal); return pending.promise;
  } }));
  const one = scope.entityResolver("document", docId);
  const two = scope.entityResolver("document", userId);
  scope.dispose();
  assert.ok(signals.every((s) => s.aborted));
  pending.resolve(doc);
  assert.equal(await one, null); assert.equal(await two, null);
  assert.deepEqual(await scope.mentionItems(""), []);
  assert.equal(await scope.entityResolver("document", docId), null);
});

test("401 terminates scope and suppresses other late authorized metadata", async () => {
  const pending = deferred<Schema["DocumentMetaResponse"]>(); let signal!: AbortSignal;
  const scope = createWorkspaceEditorEntities(ws, transport({
    documentUuid: async (_id, s) => { signal = s; return pending.promise; },
    members: async () => { throw new ProblemError(401); },
  }));
  const result = scope.entityResolver("document", docId);
  assert.deepEqual(await scope.mentionItems(""), []);
  assert.equal(signal.aborted, true);
  pending.resolve({ ...doc, title: "must not appear" } as unknown as Schema["DocumentMetaResponse"]);
  assert.equal(await result, null);
});

test("actual openapi transport forwards AbortSignals and both lexical limits=50 without cursor", async () => {
  const original = globalThis.fetch;
  const originalGet = api.GET;
  // Browser Requests accept relative URLs; Bun needs a test origin.
  api.GET = ((path, options) => originalGet(path, { ...options, baseUrl: "http://localhost" })) as typeof api.GET;
  const requests: Request[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const request = input as Request; requests.push(request);
    return Response.json({ items: [], nextCursor: null });
  }) as typeof fetch;
  try {
    const controller = new AbortController();
    await editorEntityTransport.search(ws, "needle", "task", controller.signal);
    await editorEntityTransport.search(ws, "needle", "document", controller.signal);
    await editorEntityTransport.documentUuid(docId, controller.signal);
    assert.equal(requests.length, 3);
    for (const request of requests.slice(0, 2)) {
      const url = new URL(request.url, "http://localhost");
      assert.equal(url.searchParams.get("mode"), "lexical");
      assert.equal(url.searchParams.get("limit"), "50");
      assert.equal(url.searchParams.has("cursor"), false);
      assert.equal(url.searchParams.has("projectId"), false);
    }
    assert.ok(requests[2]!.url.endsWith(`/api/v1/documents/${docId}`));
    controller.abort();
    assert.ok(requests.every((r) => r.signal.aborted));
  } finally { globalThis.fetch = original; api.GET = originalGet; }
});
