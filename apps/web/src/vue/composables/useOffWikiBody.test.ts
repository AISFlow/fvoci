import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Vue from "vue";
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

class ProblemError extends Error {
  constructor(readonly status: number) {
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
    pending: ReturnType<typeof deferred<BodySaveResult>>;
  }[] = [];
  const slots = new Map<string, string>();
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
    ) => {
      const pending = deferred<VersionedBody>();
      reads.push({ scope: { ...scope.value!, workspaceId, targetId, projectId }, pending });
      return pending.promise;
    },
    saveVersionedBody: (
      _workspace: string,
      _target: string,
      command: BodySaveCommand,
      projectId?: string | null,
    ) => {
      const pending = deferred<BodySaveResult>();
      writes.push({ command, projectId, pending });
      return pending.promise;
    },
  }) as typeof import("./useOffWikiBody").useOffWikiBody;
  const body = effects.run(() =>
    factory(
      () => scope.value,
      () => enabled.value,
    ),
  )!;
  return { body, reads, writes, scope, enabled, authRetired, effects, slots };
}
describe("OFF wiki HTTP lifetime", () => {
  test("unknown/ON mode starts no read and mode enable loads the actual target", async () => {
    const h = harness();
    h.enabled.value = false;
    h.reads[0]!.pending.resolve(source(original));
    await settle();
    expect(h.body.doc.value).toBeNull();
    const count = h.reads.length;
    h.scope.value = { ...original, targetId: "another" };
    expect(h.reads.length).toBe(count);
    h.enabled.value = true;
    const current = h.reads.at(-1)!;
    expect(current.scope.targetId).toBe("another");
    current.pending.resolve(source(current.scope));
    await settle();
    expect(h.body.doc.value).not.toBeNull();
    h.effects.stop();
  });
  test("A->B->A retires old reads, yet the newest A request succeeds", async () => {
    const h = harness();
    const old = h.reads[0]!;
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    const b = h.reads.at(-1)!;
    h.scope.value = { ...original };
    const fresh = h.reads.at(-1)!;
    old.pending.resolve(source(old.scope, "late original"));
    b.pending.resolve(source(b.scope, "B private"));
    await settle();
    expect(h.body.doc.value).toBeNull();
    fresh.pending.resolve(source(fresh.scope, "fresh A"));
    await settle();
    expect(JSON.stringify(h.body.draft.value!.mine)).toContain("fresh A");
    expect(JSON.stringify(h.body.draft.value!.mine)).not.toContain("B private");
    h.effects.stop();
  });
  test("ordinary metadata/actor object refresh retains draft and starts no new request", async () => {
    const h = harness();
    h.reads[0]!.pending.resolve(source(original));
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
    h.reads[0]!.pending.resolve(source(original));
    await settle();
    const draft = h.body.draft.value!;
    const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
    (paragraph.get(0) as Y.XmlText).insert(1, " mine");
    const oldSave = h.body.save();
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    const fresh = h.reads.at(-1)!;
    fresh.pending.resolve(source(fresh.scope, "B active"));
    await settle();
    const active = h.body.draft.value;
    h.writes[0]!.pending.reject(new ProblemError(403));
    expect(await oldSave).toBe(false);
    expect(h.body.draft.value).toBe(active);
    expect(h.body.error.value).toBeNull();
    expect(JSON.stringify(active!.mine)).toContain("B active");
    h.effects.stop();
  });
  test("conflict read denied by current permissions hides the draft and preserves its owned storage", async () => {
    const h = harness();
    h.reads[0]!.pending.resolve(source(original));
    await settle();
    const draft = h.body.draft.value!;
    const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
    (paragraph.get(0) as Y.XmlText).insert(1, " owned");
    const saving = h.body.save();
    h.writes[0]!.pending.reject(new ProblemError(409));
    await settle();
    h.reads.at(-1)!.pending.reject(new ProblemError(403));
    await saving;
    expect(h.body.doc.value).toBeNull();
    expect(h.slots.has(ownerKey(original))).toBe(true);
    h.scope.value = null;
    h.scope.value = { ...original };
    const reopened = h.reads.at(-1)!;
    reopened.pending.resolve(source(original));
    await settle();
    expect(JSON.stringify(h.body.draft.value!.mine)).toContain("owned");
    h.effects.stop();
  });
  test("logout and disposal retire callbacks without exposing them after a new identity", async () => {
    const h = harness();
    h.authRetired.value = true;
    h.reads[0]!.pending.resolve(source(original, "late logged out"));
    await settle();
    expect(h.body.doc.value).toBeNull();
    h.scope.value = { ...original, actorId: "B", credentialId: "session-B" };
    h.authRetired.value = false;
    const fresh = h.reads.at(-1)!;
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
  const a = h.reads.at(-1)!;
  expect(a.scope.projectId).toBe("project-A");
  a.pending.resolve(source(a.scope, "project A"));
  await settle();
  const draft = h.body.draft.value!;
  const paragraph = draft.doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
  (paragraph.get(0) as Y.XmlText).insert(1, " private");
  const saving = h.body.save();
  expect(h.writes[0]!.projectId).toBe("project-A");
  const command = h.writes[0]!.command;
  h.scope.value = { ...original, projectId: "project-B" };
  const b = h.reads.at(-1)!;
  b.pending.resolve(source(b.scope, "project B"));
  await settle();
  h.writes[0]!.pending.reject(new ProblemError(403));
  expect(await saving).toBe(false);
  expect(JSON.stringify(h.body.draft.value!.mine)).toContain("project B");
  expect(JSON.stringify(h.body.draft.value!.mine)).not.toContain("private");
  h.scope.value = { ...original, projectId: "project-A" };
  const resumed = h.reads.at(-1)!;
  resumed.pending.resolve(source(a.scope, "project A"));
  await settle();
  const retry = h.body.save();
  expect(h.writes[1]!.projectId).toBe("project-A");
  expect(h.writes[1]!.command).toEqual(command);
  expect(h.slots.has(ownerKey(a.scope))).toBe(true);
  h.writes[1]!.pending.resolve({
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
