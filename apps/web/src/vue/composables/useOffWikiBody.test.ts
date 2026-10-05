import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Vue from "vue";
import { parse } from "@vue/compiler-sfc";
import { NodeTypes } from "@vue/compiler-core";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import {
  OffWikiDraft,
  decodeUpdate,
  encodeUpdate,
  loadBody,
  ownerKey,
  type OffWikiOwner,
} from "../../features/documents/off-wiki-draft";
import type {
  BodySaveCommand,
  BodySaveResult,
  VersionedBody,
} from "../../features/documents/versioned-body-api";
import type {
  OffDraftCreateBody,
  OffDraftCreateResponse,
} from "../../features/documents/document-api";

class ProblemError extends Error {
  constructor(
    readonly status: number,
    readonly code?: string,
  ) {
    super(String(status));
  }
}
const text = readFileSync(new URL("./useOffWikiBody.ts", import.meta.url), "utf8")
  .replace(/^import[\s\S]*?from ["'][^"']+["'];\s*/gm, "")
  .replace("export function", "function");
const script = ts.transpile(text, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
const original = {
  actorId: "A",
  credentialId: "session-A",
  workspaceId: "workspace",
  targetId: "document",
};
function source(scope: OffWikiOwner, value = scope.actorId, tailSeq = "0"): VersionedBody {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: "kept" }, content: [{ type: "text", text: value }] },
    ],
  });
  try {
    return {
      targetId: scope.targetId,
      tailSeq,
      snapshotV1: encodeUpdate(Y.encodeStateAsUpdate(doc)),
      tailV1: [],
      contentJson: yDocToTiptapJson(doc),
      writable: true,
    };
  } finally {
    doc.destroy();
  }
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
async function settle() {
  await Promise.resolve();
  await Vue.nextTick();
  await Promise.resolve();
}
function harness() {
  const scope = Vue.shallowRef<OffWikiOwner | null>({ ...original });
  const enabled = Vue.ref(true),
    authRetired = Vue.ref(false);
  const reads: { scope: OffWikiOwner; pending: ReturnType<typeof deferred<VersionedBody>> }[] = [];
  const writes: {
    command: BodySaveCommand;
    projectId?: string | null;
    kind?: "document" | "task";
    pending: ReturnType<typeof deferred<BodySaveResult>>;
  }[] = [];
  const slots = new Map<string, string>();
  const creates: {
    body: OffDraftCreateBody;
    projectId: string | null;
    pending: ReturnType<typeof deferred<OffDraftCreateResponse>>;
  }[] = [];
  const effects = Vue.effectScope();
  const factory = runInNewContext(`${script}\nuseOffWikiBody`, {
    ...Vue,
    Y,
    OffWikiDraft,
    decodeUpdate,
    encodeUpdate,
    loadBody,
    ownerKey,
    ProblemError,
    sourceDraftAuthRetiredKey: Symbol(),
    inject: () => authRetired,
    window: {
      sessionStorage: {
        getItem: (key: string) => slots.get(key) ?? null,
        setItem: (key: string, value: string) => slots.set(key, value),
        removeItem: (key: string) => slots.delete(key),
      },
      addEventListener: () => {},
      removeEventListener: () => {},
    },
    AbortController,
    readVersionedBody: (
      workspaceId: string,
      targetId: string,
      _signal?: AbortSignal,
      projectId?: string | null,
      kind?: "document" | "task",
    ) => {
      const pending = deferred<VersionedBody>();
      reads.push({
        scope: { ...required(scope.value), workspaceId, targetId, projectId, kind },
        pending,
      });
      return pending.promise;
    },
    saveVersionedBody: (
      _workspace: string,
      _target: string,
      command: BodySaveCommand,
      projectId?: string | null,
      kind?: "document" | "task",
    ) => {
      const pending = deferred<BodySaveResult>();
      writes.push({ command, projectId, kind, pending });
      return pending.promise;
    },
    createDocumentFromDraft: (
      _workspace: string,
      projectId: string | null,
      body: OffDraftCreateBody,
    ) => {
      const pending = deferred<OffDraftCreateResponse>();
      creates.push({ body, projectId, pending });
      return pending.promise;
    },
  }) as typeof import("./useOffWikiBody").useOffWikiBody;
  const body = required(
    effects.run(() =>
      factory(
        () => scope.value,
        () => enabled.value,
      ),
    ),
  );
  return { body, reads, writes, creates, scope, enabled, authRetired, effects, slots };
}
describe("OFF wiki HTTP lifetime", () => {
  test("distinct creation unknown finish has no observer and retries exact body while keeping mine", async () => {
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const first = h.body.createDistinct({ projectId: null, parentId: null, title: "copy" });
    required(h.creates[0]).pending.reject(new ProblemError(503));
    expect(await first).toBeNull();
    expect(h.reads).toHaveLength(1);
    const retry = h.body.createDistinct({ projectId: null, parentId: null, title: "form changed" });
    expect(required(h.creates[1]).body).toBe(required(h.creates[0]).body);
    const captured = required(h.creates[1]).body;
    required(h.creates[1]).pending.resolve({
      commandId: captured.commandId,
      tailSeq: "1",
      revisionId: "33333333-3333-4333-8333-333333333333",
      document: {
        id: "22222222-2222-4222-8222-222222222222",
        workspaceId: original.workspaceId,
        parentId: null,
        projectId: null,
      },
    } as OffDraftCreateResponse);
    expect((await retry)?.document.id).toBe("22222222-2222-4222-8222-222222222222");
    expect(h.reads).toHaveLength(1);
    expect(h.body.draft.value?.start.tailSeq).toBe("0");
    h.effects.stop();
  });
  test("definite copy input refusal permits a corrected new logical request without erasing mine or observing commit", async () => {
    for (const status of [400, 413]) {
      const h = harness();
      required(h.reads[0]).pending.resolve(source(original));
      await settle();
      const draft = required(h.body.draft.value);
      const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
      (paragraph.get(0) as Y.XmlText).insert(0, "private mine ");
      const refused = h.body.createDistinct({
        projectId: null,
        parentId: null,
        title: "refused copy",
      });
      const oldCommand = required(h.creates[0]).body.commandId;
      required(h.creates[0]).pending.reject(
        new ProblemError(
          status,
          status === 400 ? "invalid_input" : "document_body_exceeds_document_max_body_bytes",
        ),
      );
      expect(await refused).toBeNull();
      expect(h.body.draft.value).toBe(draft);
      expect(h.body.pendingDistinct.value).toBeNull();
      expect(JSON.stringify(draft.mine)).toContain("private mine");
      expect(h.reads).toHaveLength(1);
      const next = h.body.createDistinct({
        projectId: null,
        parentId: null,
        title: "corrected copy",
      });
      expect(required(h.creates[1]).body.commandId).not.toBe(oldCommand);
      expect(required(h.creates[1]).body.title).toBe("corrected copy");
      required(h.creates[1]).pending.reject(new ProblemError(503));
      await next;
      expect(h.body.pendingDistinct.value?.body.commandId).toBe(
        required(h.creates[1]).body.commandId,
      );
      h.effects.stop();
    }
  });
  test("untyped HTTP input status cannot discard an unknown copy binding", async () => {
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const first = h.body.createDistinct({ projectId: null, parentId: null, title: "captured" });
    const captured = required(h.creates[0]).body;
    required(h.creates[0]).pending.reject(new ProblemError(400));
    await first;
    expect(h.body.pendingDistinct.value?.body.commandId).toBe(captured.commandId);
    const retry = h.body.createDistinct({
      projectId: null,
      parentId: null,
      title: "different form",
    });
    expect(required(h.creates[1]).body).toBe(captured);
    required(h.creates[1]).pending.reject(new ProblemError(503));
    await retry;
    expect(h.reads).toHaveLength(1);
    h.effects.stop();
  });
  test("current copy denial hides private mine and frozen command until fresh same-owner authorization", async () => {
    for (const status of [401, 403, 404]) {
      const h = harness();
      required(h.reads[0]).pending.resolve(source(original));
      await settle();
      const draft = required(h.body.draft.value);
      const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
      (paragraph.get(0) as Y.XmlText).insert(0, "owned secret ");
      const pending = h.body.createDistinct({
        projectId: null,
        parentId: null,
        title: "private copy",
      });
      const command = required(h.creates[0]).body;
      required(h.creates[0]).pending.reject(new ProblemError(status));
      expect(await pending).toBeNull();
      expect(h.body.doc.value).toBeNull();
      expect(h.body.pendingDistinct.value).toBeNull();
      expect(h.reads).toHaveLength(1);
      expect(h.slots.has(ownerKey(original))).toBe(true);
      h.scope.value = null;
      h.scope.value = { ...original };
      const fresh = required(h.reads.at(-1));
      expect(h.body.doc.value).toBeNull();
      fresh.pending.resolve(source(original));
      await settle();
      expect(JSON.stringify(required(h.body.draft.value).mine)).toContain("owned secret");
      const retry = h.body.createDistinct({
        projectId: null,
        parentId: null,
        title: "changed title",
      });
      expect(required(h.creates[1]).body).toEqual(command);
      required(h.creates[1]).pending.reject(new ProblemError(503));
      await retry;
      h.effects.stop();
    }
  });
  test("late distinct creation callback cannot expose a result to a new actor and normal requests still progress", async () => {
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const first = h.body.createDistinct({ projectId: null, parentId: null, title: "private A" });
    const captured = required(h.creates[0]).body;
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    const fresh = required(h.reads.at(-1));
    fresh.pending.resolve(source(fresh.scope, "B body"));
    await settle();
    required(h.creates[0]).pending.resolve({
      commandId: captured.commandId,
      tailSeq: "1",
      revisionId: "33333333-3333-4333-8333-333333333333",
      document: {
        id: "22222222-2222-4222-8222-222222222222",
        workspaceId: original.workspaceId,
        parentId: null,
        projectId: null,
      },
    } as OffDraftCreateResponse);
    expect(await first).toBeNull();
    expect(JSON.stringify(h.body.draft.value?.mine)).toContain("B body");
    expect(h.body.pendingDistinct.value).toBeNull();
    const next = h.body.createDistinct({ projectId: null, parentId: null, title: "B copy" });
    expect(required(h.creates[1]).body.commandId).not.toBe(captured.commandId);
    required(h.creates[1]).pending.reject(new ProblemError(503));
    await next;
    expect(h.body.doc.value).not.toBeNull();
    h.effects.stop();
  });
  test("unknown/ON mode starts no read and mode enable loads the actual target", async () => {
    const h = harness();
    h.enabled.value = false;
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    expect(h.body.doc.value).toBeNull();
    const count = h.reads.length;
    h.scope.value = { ...original, targetId: "another" };
    expect(h.reads.length).toBe(count);
    h.enabled.value = true;
    const current = required(h.reads.at(-1));
    expect(current.scope.targetId).toBe("another");
    current.pending.resolve(source(current.scope));
    await settle();
    expect(h.body.doc.value).not.toBeNull();
    h.effects.stop();
  });
  test("A->B->A retires old reads, yet the newest A request succeeds", async () => {
    const h = harness();
    const old = required(h.reads[0]);
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    const b = required(h.reads.at(-1));
    h.scope.value = { ...original };
    const fresh = required(h.reads.at(-1));
    old.pending.resolve(source(old.scope, "late original"));
    b.pending.resolve(source(b.scope, "B private"));
    await settle();
    expect(h.body.doc.value).toBeNull();
    fresh.pending.resolve(source(fresh.scope, "fresh A"));
    await settle();
    expect(JSON.stringify(required(h.body.draft.value).mine)).toContain("fresh A");
    expect(JSON.stringify(required(h.body.draft.value).mine)).not.toContain("B private");
    h.effects.stop();
  });
  test("ordinary metadata/actor object refresh retains draft and starts no new request", async () => {
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const draft = h.body.draft.value;
    h.scope.value = { ...original };
    await settle();
    expect(h.body.draft.value).toBe(draft);
    expect(h.reads.length).toBe(1);
    h.effects.stop();
  });
  test("late save denial cannot hide another owner, and its normal new request still works", async () => {
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const draft = required(h.body.draft.value);
    const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
    (paragraph.get(0) as Y.XmlText).insert(1, " mine");
    const oldSave = h.body.save();
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    const fresh = required(h.reads.at(-1));
    fresh.pending.resolve(source(fresh.scope, "B active"));
    await settle();
    const active = h.body.draft.value;
    required(h.writes[0]).pending.reject(new ProblemError(403));
    expect(await oldSave).toBe(false);
    expect(h.body.draft.value).toBe(active);
    expect(h.body.error.value).toBeNull();
    expect(JSON.stringify(required(active).mine)).toContain("B active");
    h.effects.stop();
  });
  test("conflict read denied by current permissions hides the draft and preserves its owned storage", async () => {
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const draft = required(h.body.draft.value);
    const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
    (paragraph.get(0) as Y.XmlText).insert(1, " owned");
    const saving = h.body.save();
    required(h.writes[0]).pending.reject(new ProblemError(409));
    await settle();
    required(h.reads.at(-1)).pending.reject(new ProblemError(403));
    await saving;
    expect(h.body.doc.value).toBeNull();
    expect(h.slots.has(ownerKey(original))).toBe(true);
    h.scope.value = null;
    h.scope.value = { ...original };
    const reopened = required(h.reads.at(-1));
    reopened.pending.resolve(source(original));
    await settle();
    expect(JSON.stringify(required(h.body.draft.value).mine)).toContain("owned");
    h.effects.stop();
  });
  test("logout and disposal retire callbacks without exposing them after a new identity", async () => {
    const h = harness();
    h.authRetired.value = true;
    required(h.reads[0]).pending.resolve(source(original, "late logged out"));
    await settle();
    expect(h.body.doc.value).toBeNull();
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    h.authRetired.value = false;
    const fresh = required(h.reads.at(-1));
    fresh.pending.resolve(source(fresh.scope));
    await settle();
    expect(h.body.doc.value).not.toBeNull();
    h.effects.stop();
    expect(h.body.doc.value).toBeNull();
  });
});

test("project affiliation changes retire reads and preserve separate drafts and retry bindings", async () => {
  const h = harness();
  h.scope.value = { ...original, projectId: "project-A" };
  const a = required(h.reads.at(-1));
  expect(a.scope.projectId).toBe("project-A");
  a.pending.resolve(source(a.scope, "project A"));
  await settle();
  const draft = required(h.body.draft.value);
  const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
  (paragraph.get(0) as Y.XmlText).insert(1, " private");
  const saving = h.body.save();
  expect(required(h.writes[0]).projectId).toBe("project-A");
  const command = required(h.writes[0]).command;
  h.scope.value = { ...original, projectId: "project-B" };
  const b = required(h.reads.at(-1));
  b.pending.resolve(source(b.scope, "project B"));
  await settle();
  required(h.writes[0]).pending.reject(new ProblemError(403));
  expect(await saving).toBe(false);
  expect(JSON.stringify(required(h.body.draft.value).mine)).toContain("project B");
  expect(JSON.stringify(required(h.body.draft.value).mine)).not.toContain("private");
  h.scope.value = { ...original, projectId: "project-A" };
  const resumed = required(h.reads.at(-1));
  resumed.pending.resolve(source(a.scope, "project A"));
  await settle();
  const retry = h.body.save();
  expect(required(h.writes[1]).projectId).toBe("project-A");
  expect(required(h.writes[1]).command).toEqual(command);
  expect(h.slots.has(ownerKey(a.scope))).toBe(true);
  required(h.writes[1]).pending.resolve({
    commandId: command.commandId,
    targetId: original.targetId,
    tailSeq: "1",
    revisionId: "revision",
  });
  expect(await retry).toBe(true);
  expect(h.slots.has(ownerKey(a.scope))).toBe(false);
  expect(ownerKey(a.scope)).not.toBe(ownerKey(b.scope));
  h.effects.stop();
});

test("task native reads, saves and conflict refresh bind task kind without borrowing a document draft", async () => {
  const h = harness();
  h.scope.value = { ...original, kind: "task", projectId: "owning-project" };
  const read = required(h.reads.at(-1));
  expect(read.scope.kind).toBe("task");
  read.pending.resolve(source(read.scope, "task private"));
  await settle();
  const draft = required(h.body.draft.value);
  const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
  (paragraph.get(0) as Y.XmlText).insert(1, " task owned edit");
  const saving = h.body.save();
  expect(required(h.writes[0]).kind).toBe("task");
  required(h.writes[0]).pending.reject(new ProblemError(409));
  await settle();
  const conflict = required(h.reads.at(-1));
  expect(conflict.scope.kind).toBe("task");
  conflict.pending.resolve(source(read.scope, "task latest", "1"));
  expect(await saving).toBe(false);
  expect(h.body.comparison.value).not.toBeNull();
  expect(JSON.stringify(required(h.body.comparison.value).mine)).toContain("task owned edit");
  expect(JSON.stringify(required(h.body.comparison.value).current)).toContain("task latest");
  h.scope.value = { ...original };
  const docRead = required(h.reads.at(-1));
  docRead.pending.resolve(source(original, "document history"));
  await settle();
  expect(JSON.stringify(required(h.body.draft.value).mine)).toContain("document history");
  expect(JSON.stringify(required(h.body.draft.value).mine)).not.toContain("task owned edit");
  expect(ownerKey(read.scope)).not.toBe(ownerKey(original));
  h.effects.stop();
});

for (const host of [
  "documents/WikiDocumentView.vue",
  "documents/ProjectDocumentView.vue",
  "tasks/TaskBodyEditor.vue",
]) {
  test(`actual ${host} copy button retries frozen intent with retained Markdown but blocks new intent`, async () => {
    const filename = new URL(`../features/${host}`, import.meta.url);
    const { descriptor, errors } = parse(readFileSync(filename, "utf8"));
    expect(errors).toEqual([]);
    const nodes = [...required(required(descriptor.template).ast).children].reverse();
    const disabledExpressions: string[] = [];
    while (nodes.length) {
      const node = required(nodes.pop());
      if (node.type !== NodeTypes.ELEMENT) continue;
      nodes.push(...[...node.children].reverse());
      if (node.tag !== "UButton") continue;
      const click = node.props.find(
        (prop) =>
          prop.type === NodeTypes.DIRECTIVE &&
          prop.name === "on" &&
          prop.arg?.type === NodeTypes.SIMPLE_EXPRESSION &&
          prop.arg.content === "click" &&
          prop.exp?.type === NodeTypes.SIMPLE_EXPRESSION &&
          prop.exp.content === "copyOffDraft",
      );
      if (!click) continue;
      const disabled = node.props.find(
        (prop) =>
          prop.type === NodeTypes.DIRECTIVE &&
          prop.name === "bind" &&
          prop.arg?.type === NodeTypes.SIMPLE_EXPRESSION &&
          prop.arg.content === "disabled",
      );
      if (
        disabled?.type === NodeTypes.DIRECTIVE &&
        disabled.exp?.type === NodeTypes.SIMPLE_EXPRESSION
      )
        disabledExpressions.push(disabled.exp.content);
    }
    expect(disabledExpressions).toHaveLength(2);
    // First is the actual comparison-section button; the separate unknown
    // retry section cannot replace its reachable enabled state.
    const disabledExpression = required(disabledExpressions[0]);
    const setup = ts.createSourceFile(
      host,
      required(descriptor.scriptSetup).content,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TS,
    );
    const copy = setup.statements.find(
      (node) => ts.isFunctionDeclaration(node) && node.name?.text === "copyOffDraft",
    );
    expect(copy).toBeDefined();
    const copyScript = ts.transpile(required(copy).getText(setup), {
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.None,
    });
    const h = harness();
    required(h.reads[0]).pending.resolve(source(original));
    await settle();
    const draft = required(h.body.draft.value);
    const sourceDraftState = Vue.shallowRef({ dirty: true, composing: false });
    const retainedBuffer = {
      text: "new unapplied private Markdown 😀",
      baseV1: Y.encodeStateAsUpdate(draft.doc),
    };
    draft.setSourceBuffer(retainedBuffer);
    const environment = {
      offBody: h.body,
      props: { offBody: h.body, workspaceId: original.workspaceId },
      sourceDraft: sourceDraftState,
      realtimeOff: { value: true },
      persistLifecycle: { value: 1 },
      copyParentId: { value: "live-parent" },
      copyProjectId: { value: "live-project" },
      copyDestination: { value: "project" },
      copyTitle: { value: "changed form must not replace frozen body" },
      scope: { value: { projectId: null } },
      meta: { value: { parentId: null, title: "source" } },
      copiedDraft: { value: null },
      projectId: { value: "live-project" },
      t: (key: string) => key,
      queryClient: { invalidateQueries: async () => {} },
      projectDocumentsQuery: () => ({ queryKey: ["live-tree"] }),
    };
    const invoke = runInNewContext(
      `${copyScript}\ncopyOffDraft`,
      environment,
    ) as () => Promise<void>;
    const disabled = (
      pending: boolean,
      dirty: boolean,
      composing: boolean,
      creating = false,
      saving = false,
    ) =>
      runInNewContext(disabledExpression, {
        offBody: {
          pendingDistinct: { value: pending ? { body: { commandId: "frozen" } } : null },
          creating: { value: creating },
          saving: { value: saving },
        },
        sourceDraft: { dirty, composing },
        copyParentId: "live-parent",
        copyProjectId: "live-project",
        copyDestination: "project",
      }) as boolean;
    for (const state of [
      { dirty: true, composing: false },
      { dirty: false, composing: true },
    ]) {
      sourceDraftState.value = state;
      expect(disabled(false, state.dirty, state.composing)).toBe(true);
      await invoke();
      expect(h.creates).toHaveLength(0);
      expect(draft.sourceBuffer?.text).toBe(retainedBuffer.text);
    }
    expect(disabled(false, false, false)).toBe(false);
    draft.setSourceBuffer(null);
    const initial = h.body.createDistinct({
      projectId: null,
      parentId: null,
      title: "frozen private copy",
    });
    const frozen = required(h.creates[0]).body;
    required(h.creates[0]).pending.reject(new ProblemError(503));
    expect(await initial).toBeNull();
    draft.conflict(source(original, "current body", "1"));
    expect(h.body.comparison.value).not.toBeNull();
    draft.setSourceBuffer(retainedBuffer);
    sourceDraftState.value = { dirty: true, composing: true };
    expect(disabled(true, true, false)).toBe(false);
    expect(disabled(true, false, true)).toBe(false);
    expect(disabled(true, true, true)).toBe(false);
    expect(disabled(true, true, true, true)).toBe(true);
    expect(disabled(true, true, true, false, true)).toBe(true);
    const retry = invoke();
    expect(h.creates).toHaveLength(2);
    expect(h.body.creating.value).toBe(true);
    await invoke();
    expect(h.creates).toHaveLength(2);
    expect(required(h.creates[1]).body).toBe(frozen);
    expect(required(h.creates[1]).projectId).toBeNull();
    expect(draft.sourceBuffer?.text).toBe(retainedBuffer.text);
    required(h.creates[1]).pending.resolve({
      commandId: frozen.commandId,
      tailSeq: "1",
      revisionId: "33333333-3333-4333-8333-333333333333",
      document: {
        id: "22222222-2222-4222-8222-222222222222",
        workspaceId: original.workspaceId,
        parentId: null,
        projectId: null,
      },
    } as OffDraftCreateResponse);
    await retry;
    expect(h.body.pendingDistinct.value).toBeNull();
    expect(draft.sourceBuffer?.text).toBe(retainedBuffer.text);
    expect(draft.start.tailSeq).toBe("0");
    expect(h.body.comparison.value).not.toBeNull();
    h.effects.stop();
  });
}

function required<T>(value: T | null | undefined): T {
  if (value == null) throw new Error("Missing required test fixture value");
  return value;
}
