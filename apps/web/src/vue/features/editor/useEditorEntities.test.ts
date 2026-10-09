import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import test from "node:test";
import ts from "typescript";
import { computed, effectScope, ref, shallowReactive, shallowRef } from "vue";
import { compileScript, parse } from "vue/compiler-sfc";
import type { components } from "@/generated/api";
import type { EditorEntityTransport } from "@/features/workspace/editor-entities";
import { useEditorEntities } from "./useEditorEntities";

const id = "22222222-2222-4222-8222-222222222222";
type Doc = components["schemas"]["DocumentMetaResponse"];
function doc(workspaceId: string): Doc {
  return {
    id,
    workspaceId,
    title: "Authorized",
    icon: "📄",
    projectId: null,
    number: 1,
    parentId: null,
    path: "",
    status: "draft",
    schemaVersion: 2,
    sortKey: "",
    version: 1,
    createdAt: "",
    updatedAt: "",
    createdBy: id,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((ok) => {
    resolve = ok;
  });
  return { promise, resolve };
}
function transport(documentUuid: EditorEntityTransport["documentUuid"]): EditorEntityTransport {
  const unused = (): Promise<never> => Promise.reject(new Error("unexpected endpoint"));
  return {
    documentUuid,
    members: unused,
    groups: unused,
    projects: unused,
    lookup: unused,
    search: unused,
    document: unused,
    task: unused,
    workflow: unused,
  };
}

await test("scope teardown aborts real requests and stable callbacks cannot enrich after unmount", async () => {
  const pending = deferred<Doc>();
  let signal!: AbortSignal;
  const scope = effectScope();
  const callbacks = required(
    scope.run(() =>
      useEditorEntities(
        () => "ws",
        () => "room",
        transport(async (_id, s) => {
          signal = s;
          return pending.promise;
        }),
      ),
    ),
  );
  const result = callbacks.entityResolver("document", id);
  scope.stop();
  assert.equal(signal.aborted, true);
  pending.resolve(doc("ws"));
  assert.equal(await result, null);
  assert.equal(await callbacks.entityResolver("document", id), null);
  assert.deepEqual(await callbacks.mentionItems(""), []);
});

await test("workspace and room/session changes retire old requests synchronously without replacing callback identities", async () => {
  const workspace = ref("old");
  const room = ref("doc:1:user-a");
  const pending: ReturnType<typeof deferred<Doc>>[] = [];
  const signals: AbortSignal[] = [];
  const scope = effectScope();
  const callbacks = required(
    scope.run(() =>
      useEditorEntities(
        () => workspace.value,
        () => room.value,
        transport(async (_id, signal) => {
          const result = deferred<Doc>();
          pending.push(result);
          signals.push(signal);
          return result.promise;
        }),
      ),
    ),
  );
  const resolver = callbacks.entityResolver;
  const mentions = callbacks.mentionItems;
  const old = resolver("document", id);
  workspace.value = "new";
  assert.equal(required(signals[0]).aborted, true);
  required(pending[0]).resolve(doc("old"));
  assert.equal(await old, null);
  const retiredRoom = resolver("document", id);
  room.value = "doc:2:user-b";
  assert.equal(required(signals[1]).aborted, true);
  required(pending[1]).resolve(doc("new"));
  assert.equal(await retiredRoom, null);
  const current = resolver("document", id);
  required(pending[2]).resolve(doc("new"));
  assert.equal((await current)?.label, "Authorized");
  assert.equal(callbacks.entityResolver, resolver);
  assert.equal(callbacks.mentionItems, mentions);
  scope.stop();
});

await test("remount gets fresh metadata and cannot revive an old callback", async () => {
  const first = effectScope();
  let calls = 0;
  const api = transport(() => {
    calls++;
    return Promise.resolve(doc("ws"));
  });
  const one = required(
    first.run(() =>
      useEditorEntities(
        () => "ws",
        () => "room",
        api,
      ),
    ),
  );
  assert.ok(await one.entityResolver("document", id));
  first.stop();
  const second = effectScope();
  const two = required(
    second.run(() =>
      useEditorEntities(
        () => "ws",
        () => "room",
        api,
      ),
    ),
  );
  assert.ok(await two.entityResolver("document", id));
  assert.equal(await one.entityResolver("document", id), null);
  assert.equal(calls, 2);
  second.stop();
});

await test("all three compiled consumers supply callbacks before their one existing editor mount", () => {
  for (const rel of [
    "../documents/WikiDocumentView.vue",
    "../documents/ProjectDocumentView.vue",
    "../tasks/TaskBodyEditor.vue",
  ]) {
    const filename = new URL(rel, import.meta.url).pathname;
    const source = readFileSync(filename, "utf8");
    const { descriptor, errors } = parse(source, { filename });
    assert.deepEqual(errors, []);
    const compiled = compileScript(descriptor, { id: rel, inlineTemplate: true }).content;
    assert.match(compiled, /useEditorEntities\(/);
    assert.match(compiled, /"mention-items": _unref\(mentionItems\)/);
    assert.match(compiled, /"entity-resolver": _unref\(entityResolver\)/);
    assert.equal(source.match(/<FvociEditor\b/g)?.length, 1);
    assert.match(source, /:key="bodyGeneration"/);
    assert.match(source, /:ydoc="bodyDoc"/);
    assert.match(source, /:provider="session\?\.provider"/);
    assert.equal(source.includes("new Y.Doc"), false);
    assertBodyIdentity(source);
  }
});

function bodyIdentityDeclarations(source: string) {
  const { descriptor } = parse(source);
  assert.ok(descriptor.scriptSetup);
  const script = ts.createSourceFile(
    "host.ts",
    descriptor.scriptSetup.content,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const declarations = script.statements.filter(
    (node) =>
      ts.isVariableStatement(node) &&
      node.declarationList.declarations.some(
        (declaration) =>
          ts.isIdentifier(declaration.name) &&
          ["bodyDoc", "bodyGeneration"].includes(declaration.name.text),
      ),
  );
  assert.equal(declarations.length, 2);
  return declarations.map((node) => node.getText(script));
}

// Execute the real selectors: session snapshot replacement is not retirement,
// while document identity and body generation select the current ON/OFF owner.
function assertBodyIdentity(source: string) {
  const onDoc = {},
    offDoc = {};
  const session = shallowRef<{ doc: object; generation: number } | null>({
    doc: onDoc,
    generation: 7,
  });
  const offBody = { doc: shallowRef(offDoc), generation: ref(3) };
  const realtimeOff = ref(false);
  const props = shallowReactive({ session: session.value, offBody: null as typeof offBody | null });
  const selectors = runInNewContext(
    new Bun.Transpiler({ loader: "ts" }).transformSync(
      `(() => {${bodyIdentityDeclarations(source).join("\n")}; return {bodyDoc, bodyGeneration};})()`,
    ),
    { computed, session, offBody, realtimeOff, props },
  ) as { bodyDoc: { value: unknown }; bodyGeneration: { value: unknown } };
  assert.equal(selectors.bodyDoc.value, onDoc);
  assert.equal(selectors.bodyGeneration.value, 7);
  session.value = { doc: onDoc, generation: 7 };
  props.session = session.value;
  assert.equal(selectors.bodyDoc.value, onDoc);
  assert.equal(selectors.bodyGeneration.value, 7);
  const nextOnDoc = {};
  session.value = { doc: nextOnDoc, generation: 8 };
  props.session = session.value;
  assert.equal(selectors.bodyDoc.value, nextOnDoc);
  assert.equal(selectors.bodyGeneration.value, 8);
  realtimeOff.value = true;
  props.offBody = offBody;
  session.value = null;
  props.session = null;
  assert.equal(selectors.bodyDoc.value, offDoc);
  assert.equal(selectors.bodyGeneration.value, "off:3");
  const nextOffDoc = {};
  offBody.doc.value = nextOffDoc;
  offBody.generation.value = 4;
  assert.equal(selectors.bodyDoc.value, nextOffDoc);
  assert.equal(selectors.bodyGeneration.value, "off:4");
}

await test("all three actual body selector oracles reject session-only identity and frozen generations", () => {
  for (const rel of [
    "../documents/WikiDocumentView.vue",
    "../documents/ProjectDocumentView.vue",
    "../tasks/TaskBodyEditor.vue",
  ]) {
    const source = readFileSync(new URL(rel, import.meta.url), "utf8");
    for (const name of ["bodyDoc", "bodyGeneration"]) {
      const original = required(
        bodyIdentityDeclarations(source).find((text) => text.startsWith(`const ${name} =`)),
      );
      const wrong =
        name === "bodyDoc"
          ? "const bodyDoc = computed(() => props.session?.doc ?? session.value?.doc);"
          : "const bodyGeneration = computed(() => 7);";
      const mutated = source.replace(original, wrong);
      assert.notEqual(mutated, source);
      assert.throws(
        () => {
          assertBodyIdentity(mutated);
        },
        assert.AssertionError,
        `${rel}/${name}`,
      );
    }
  }
});

function required<T>(value: T | null | undefined): T {
  assert.ok(value !== null && value !== undefined, "required fixture value");
  return value;
}
