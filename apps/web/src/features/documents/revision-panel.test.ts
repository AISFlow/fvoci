import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Vue from "vue";
import { ProblemError } from "../../lib/api";
import { persistThenCreate } from "./revision-persist.ts";

await test("manual save calls persistNow first", async () => {
  const order: string[] = [];
  await persistThenCreate(
    () => {
      order.push("persist");
    },
    () => {
      order.push("create");
    },
  );
  assert.deepEqual(order, ["persist", "create"]);
});

await test("failed persistence prevents creating a revision", async () => {
  let created = false;
  await assert.rejects(
    persistThenCreate(
      () => Promise.reject(new Error("save failed")),
      () => {
        created = true;
      },
    ),
    /save failed/,
  );
  assert.equal(created, false);
});

await test("unavailable persist barrier fails explicitly instead of creating stale history", async () => {
  let created = false;
  await assert.rejects(
    persistThenCreate(undefined, () => {
      created = true;
    }),
    /live persist barrier/,
  );
  assert.equal(created, false);
});

await test("persistThenCreate stays pending until create completes and propagates its failure", async () => {
  let reject!: (error: Error) => void;
  const creating = new Promise<void>((_resolve, no) => {
    reject = no;
  });
  let settled = false;
  const saving = persistThenCreate(
    () => Promise.resolve(),
    () => creating,
  );
  void saving
    .finally(() => {
      settled = true;
    })
    .catch(() => undefined);
  await Promise.resolve();
  assert.equal(settled, false);
  reject(new Error("create failed"));
  await assert.rejects(saving, /create failed/);
});

function hostPersist(file: string, available: boolean, delayed = false) {
  const source = readFileSync(new URL(file, import.meta.url), "utf8");
  const script = source.slice(
    source.indexOf('<script setup lang="ts">') + 24,
    source.indexOf("</script>"),
  );
  const parsed = ts.createSourceFile(
    "host.ts",
    script,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const fn = parsed.statements.find(
    (node) => ts.isFunctionDeclaration(node) && node.name?.text === "persistBody",
  );
  assert.ok(fn);
  let called = 0;
  let resolve!: () => void;
  let reject!: (reason: unknown) => void;
  const pending = new Promise<void>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  const current = Vue.markRaw({
    persistNow: () => {
      called++;
      return delayed ? pending : Promise.resolve();
    },
    status: "connected",
    doc: {},
    provider: {},
    generation: 0,
  });
  const session = Vue.shallowRef(current);
  const props = Vue.shallowReactive({
    session: current,
    workspaceId: "ws",
    taskId: "task",
    collabUser: { id: "actor" },
  });
  const actor = Vue.shallowRef({ userId: "actor", sessionId: "session" });
  const meError = Vue.shallowRef<unknown>(null);
  const resource = Vue.ref({ workspaceId: "ws", documentId: "doc", projectId: null });
  const readOnly = Vue.ref(false);
  const persisting = Vue.ref(false);
  const persistError = Vue.ref<string | null>(null);
  const realtimeOff = Vue.ref(false);
  const offBody = { doc: Vue.shallowRef({}), generation: Vue.ref(1) };
  const context = {
    ...Vue,
    session,
    props,
    scope: resource,
    readOnly,
    persisting,
    persistError,
    me: { data: actor, error: meError },
    ProblemError,
    canPersist: Vue.ref(available),
    realtimeOff,
    offBody,
    t: (key: string) => key,
    Error,
  };
  // Execute only the actual persist owner and its retirement subscriptions.
  // Unrelated source-draft/export helpers between these declarations are not
  // dependencies of persistBody and must not become accidental VM imports.
  const bodyIdentity = parsed.statements.filter(
    (node) =>
      ts.isVariableStatement(node) &&
      node.declarationList.declarations.some(
        (declaration) =>
          ts.isIdentifier(declaration.name) &&
          ["bodyDoc", "bodyGeneration"].includes(declaration.name.text),
      ),
  );
  assert.equal(bodyIdentity.length, 2, "actual selected body identity and generation");
  const lifetime = parsed.statements.filter((node) => {
    if (ts.isVariableStatement(node))
      return node.declarationList.declarations.some(
        (declaration) =>
          ts.isIdentifier(declaration.name) && declaration.name.text === "persistLifecycle",
      );
    return (
      ts.isExpressionStatement(node) &&
      ts.isCallExpression(node.expression) &&
      ["watch", "onScopeDispose"].includes(node.expression.expression.getText(parsed)) &&
      node.expression.arguments.some((argument) =>
        argument.getText(parsed).includes("persistLifecycle.value++"),
      )
    );
  });
  assert.equal(lifetime.length, 3, "actual lifetime ref, synchronous watcher and disposal");
  const owner = Vue.effectScope();
  const callable: unknown = owner.run((): unknown =>
    runInNewContext(
      new Bun.Transpiler({ loader: "ts" }).transformSync(
        `(() => {${[...bodyIdentity, ...lifetime].map((node) => node.getText(parsed)).join("\n")};${fn.getText(parsed)}; return {persistBody, persistLifecycle};})()`,
      ),
      context,
    ),
  );
  assert.ok(callable !== null && typeof callable === "object");
  const callbacks = callable as {
    persistBody: () => Promise<void>;
    persistLifecycle: Vue.Ref<number>;
  };
  assert.ok(typeof callbacks.persistBody === "function");
  return {
    run: callbacks.persistBody,
    lifetime: () => callbacks.persistLifecycle.value,
    calls: () => called,
    resolve,
    reject,
    persisting,
    persistError,
    stop: () => {
      owner.stop();
    },
    retire(change: string) {
      if (change === "actor-aba") {
        actor.value = { ...actor.value, userId: "other" };
        actor.value = { ...actor.value, userId: "actor" };
        props.collabUser = { id: "other" };
        props.collabUser = { id: "actor" };
      }
      if (change === "on-doc" || change === "on-generation") {
        const replacement = Vue.markRaw({
          ...session.value,
          doc: change === "on-doc" ? {} : session.value.doc,
          generation: session.value.generation + (change === "on-generation" ? 1 : 0),
        });
        session.value = replacement;
        props.session = replacement;
      }
      if (change === "off") {
        realtimeOff.value = true;
        Object.assign(props, { offBody });
      }
      if (change === "off-doc") offBody.doc.value = {};
      if (change === "off-generation") offBody.generation.value++;
      if (change === "aba") {
        resource.value = { ...resource.value, documentId: "other" };
        resource.value = { ...resource.value, documentId: "doc" };
        props.taskId = "other";
        props.taskId = "task";
      }
      if (change === "session-aba") {
        actor.value = { ...actor.value, sessionId: "other" };
        actor.value = { ...actor.value, sessionId: "session" };
      }
      if (change === "signed-out") meError.value = new ProblemError(401);
      if (change === "readonly") readOnly.value = true;
      if (change === "dispose") owner.stop();
      if (change === "snapshot") {
        const replacement = Vue.markRaw({ ...current });
        session.value = replacement;
        props.session = replacement;
      }
    },
  };
}

await test("all three editor hosts reject an unavailable persist callback rather than resolving a no-op", async () => {
  for (const file of [
    "../../vue/features/documents/WikiDocumentView.vue",
    "../../vue/features/documents/ProjectDocumentView.vue",
    "../../vue/features/tasks/TaskBodyEditor.vue",
  ]) {
    const h = hostPersist(file, false);
    await assert.rejects(
      persistThenCreate(h.run, () => assert.fail("must not create")),
      /collab persist unavailable/,
    );
    assert.equal(h.calls(), 0);
    h.stop();
  }
});

await test("all three available editor host callbacks wait for the real persist call", async () => {
  for (const file of [
    "../../vue/features/documents/WikiDocumentView.vue",
    "../../vue/features/documents/ProjectDocumentView.vue",
    "../../vue/features/tasks/TaskBodyEditor.vue",
  ]) {
    const h = hostPersist(file, true);
    await persistThenCreate(h.run, () => undefined);
    assert.equal(h.calls(), 1);
    h.stop();
  }
});

await test("actual host lifetime watchers reject late persist success/error across retirement and preserve newer UI", async () => {
  for (const file of [
    "../../vue/features/documents/WikiDocumentView.vue",
    "../../vue/features/documents/ProjectDocumentView.vue",
    "../../vue/features/tasks/TaskBodyEditor.vue",
  ])
    for (const change of [
      "aba",
      "session-aba",
      "actor-aba",
      "on-doc",
      "on-generation",
      "signed-out",
      "readonly",
      "dispose",
    ])
      for (const failure of [false, true]) {
        const h = hostPersist(file, true, true);
        try {
          const saving = h.run();
          h.retire(change);
          h.persistError.value = "new scope notice";
          h.persisting.value = true;
          const rejected = assert.rejects(saving, /collab persist|old failure/);
          if (failure) h.reject(new Error("old failure"));
          else h.resolve();
          await rejected;
          assert.equal(h.persistError.value, "new scope notice", file + "/" + change);
          assert.equal(h.persisting.value, true, file + "/" + change);
        } finally {
          h.stop();
        }
      }
});

await test("actual ON/OFF body owner transitions retire host watchers while stable snapshots do not", () => {
  for (const file of [
    "../../vue/features/documents/WikiDocumentView.vue",
    "../../vue/features/documents/ProjectDocumentView.vue",
    "../../vue/features/tasks/TaskBodyEditor.vue",
  ]) {
    const h = hostPersist(file, true);
    try {
      const initial = h.lifetime();
      h.retire("snapshot");
      assert.equal(h.lifetime(), initial);
      for (const change of [
        "on-doc",
        "on-generation",
        "off",
        "off-doc",
        "off-generation",
        "actor-aba",
        "signed-out",
      ]) {
        const previous = h.lifetime();
        h.retire(change);
        assert.ok(h.lifetime() > previous, `${file}/${change}`);
      }
    } finally {
      h.stop();
    }
  }
});

await test("ordinary host session snapshot replacements do not retire a valid persist ACK", async () => {
  for (const file of [
    "../../vue/features/documents/WikiDocumentView.vue",
    "../../vue/features/documents/ProjectDocumentView.vue",
    "../../vue/features/tasks/TaskBodyEditor.vue",
  ]) {
    const h = hostPersist(file, true, true);
    try {
      const saving = h.run();
      h.retire("snapshot");
      h.resolve();
      await saving;
      assert.equal(h.persisting.value, false);
    } finally {
      h.stop();
    }
  }
});
