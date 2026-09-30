import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { effectScope, ref } from "vue";
import { compileScript, parse } from "vue/compiler-sfc";
import type { components } from "@/generated/api";
import type { EditorEntityTransport } from "@/features/workspace/editor-entities";
import { useEditorEntities } from "./useEditorEntities";

const id = "22222222-2222-4222-8222-222222222222";
type Doc = components["schemas"]["DocumentMetaResponse"];
function doc(workspaceId: string): Doc {
  return { id, workspaceId, title: "Authorized", icon: "📄", projectId: null, number: 1,
    parentId: null, path: "", status: "draft", schemaVersion: 2, sortKey: "", version: 1,
    createdAt: "", updatedAt: "", createdBy: id };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((ok) => { resolve = ok; });
  return { promise, resolve };
}
function transport(documentUuid: EditorEntityTransport["documentUuid"]): EditorEntityTransport {
  const unused = async (): Promise<never> => { throw new Error("unexpected endpoint"); };
  return { documentUuid, members: unused, groups: unused, projects: unused, lookup: unused,
    search: unused, document: unused, task: unused, workflow: unused };
}

test("scope teardown aborts real requests and stable callbacks cannot enrich after unmount", async () => {
  const pending = deferred<Doc>(); let signal!: AbortSignal;
  const scope = effectScope();
  const callbacks = scope.run(() => useEditorEntities(() => "ws", () => "room", transport(async (_id, s) => {
    signal = s; return pending.promise;
  })))!;
  const result = callbacks.entityResolver("document", id);
  scope.stop();
  assert.equal(signal.aborted, true);
  pending.resolve(doc("ws"));
  assert.equal(await result, null);
  assert.equal(await callbacks.entityResolver("document", id), null);
  assert.deepEqual(await callbacks.mentionItems(""), []);
});

test("workspace and room/session changes retire old requests synchronously without replacing callback identities", async () => {
  const workspace = ref("old"); const room = ref("doc:1:user-a");
  const pending: ReturnType<typeof deferred<Doc>>[] = [];
  const signals: AbortSignal[] = [];
  const scope = effectScope();
  const callbacks = scope.run(() => useEditorEntities(() => workspace.value, () => room.value, transport(async (_id, signal) => {
    const result = deferred<Doc>(); pending.push(result); signals.push(signal); return result.promise;
  })))!;
  const resolver = callbacks.entityResolver; const mentions = callbacks.mentionItems;
  const old = resolver("document", id);
  workspace.value = "new";
  assert.equal(signals[0]!.aborted, true);
  pending[0]!.resolve(doc("old")); assert.equal(await old, null);
  const retiredRoom = resolver("document", id);
  room.value = "doc:2:user-b";
  assert.equal(signals[1]!.aborted, true);
  pending[1]!.resolve(doc("new")); assert.equal(await retiredRoom, null);
  const current = resolver("document", id);
  pending[2]!.resolve(doc("new")); assert.equal((await current)?.label, "Authorized");
  assert.equal(callbacks.entityResolver, resolver); assert.equal(callbacks.mentionItems, mentions);
  scope.stop();
});

test("remount gets fresh metadata and cannot revive an old callback", async () => {
  const first = effectScope(); let calls = 0;
  const api = transport(async () => { calls++; return doc("ws"); });
  const one = first.run(() => useEditorEntities(() => "ws", () => "room", api))!;
  assert.ok(await one.entityResolver("document", id)); first.stop();
  const second = effectScope();
  const two = second.run(() => useEditorEntities(() => "ws", () => "room", api))!;
  assert.ok(await two.entityResolver("document", id));
  assert.equal(await one.entityResolver("document", id), null);
  assert.equal(calls, 2); second.stop();
});

test("all three compiled consumers supply callbacks before their one existing editor mount", () => {
  for (const rel of ["../documents/WikiDocumentView.vue", "../documents/ProjectDocumentView.vue", "../tasks/TaskBodyEditor.vue"]) {
    const filename = new URL(rel, import.meta.url).pathname;
    const source = readFileSync(filename, "utf8");
    const { descriptor, errors } = parse(source, { filename }); assert.deepEqual(errors, []);
    const compiled = compileScript(descriptor, { id: rel, inlineTemplate: true }).content;
    assert.match(compiled, /useEditorEntities\(/);
    assert.match(compiled, /"mention-items": _unref\(mentionItems\)/);
    assert.match(compiled, /"entity-resolver": _unref\(entityResolver\)/);
    assert.equal(source.match(/<FvociEditor\b/g)?.length, 1);
    assert.match(source, /:key="session.generation"/);
    assert.match(source, /:ydoc="session.doc"/);
    assert.match(source, /:provider="session.provider"/);
    assert.equal(source.includes("new Y.Doc"), false);
  }
});
