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
  const props = Vue.reactive({
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
    t: (key: string) => key,
    Error,
  };
  const lifetimeStart = script.indexOf("let persistLifecycle =");
  assert.ok(lifetimeStart > 0);
  const owner = Vue.effectScope();
  const callable: unknown = owner.run(() =>
    runInNewContext(
      new Bun.Transpiler({ loader: "ts" }).transformSync(
        `(() => {${script.slice(lifetimeStart, fn.end)}; return persistBody;})()`,
      ),
      context,
    ),
  );
  assert.ok(typeof callable === "function");
  return {
    run: callable as () => Promise<void>,
    calls: () => called,
    resolve,
    reject,
    persisting,
    persistError,
    stop: () => owner.stop(),
    retire(change: string) {
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
    for (const change of ["aba", "session-aba", "signed-out", "readonly", "dispose"])
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
