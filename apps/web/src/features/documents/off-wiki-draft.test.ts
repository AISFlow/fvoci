import { describe, expect, test } from "bun:test";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { FVOCI_YDOC_FRAGMENT } from "@fvoci/editor/collab";
import { OffWikiDraft, encodeUpdate, loadBody, ownerKey } from "./off-wiki-draft";
import type { BodySaveCommand, BodySaveResult, VersionedBody } from "./versioned-body-api";

const targetId = "11111111-1111-4111-8111-111111111111";
const commandId = "22222222-2222-4222-8222-222222222222";
const revisionId = "33333333-3333-4333-8333-333333333333";
const owner = { actorId: "actor", credentialId: "credential", workspaceId: "workspace", targetId };
function storage() {
  const slots = new Map<string, string>();
  return {
    slots,
    getItem: (key: string) => slots.get(key) ?? null,
    setItem: (key: string, value: string) => {
      slots.set(key, value);
    },
    removeItem: (key: string) => {
      slots.delete(key);
    },
  };
}
function source(text = "start", tailSeq = "9007199254740993"): VersionedBody {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "kept-block" },
        content: [
          {
            type: "text",
            text,
            marks: [{ type: "bold" }, { type: "link", attrs: { href: "https://example.com" } }],
          },
        ],
      },
    ],
  });
  try {
    return {
      targetId,
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
function edit(doc: Y.Doc, text: string) {
  const paragraph = doc.getXmlFragment(FVOCI_YDOC_FRAGMENT).get(0) as Y.XmlElement;
  const run = paragraph.get(0) as Y.XmlText;
  run.insert(run.length, text);
}
function result(command: BodySaveCommand): BodySaveResult {
  return {
    commandId: command.commandId,
    targetId,
    tailSeq: String(BigInt(command.expectedTailSeq) + 1n),
    revisionId,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("OFF wiki native draft owner", () => {
  test("fresh read-only authority overrides restored write permission without discarding the owned draft", async () => {
    const persisted = storage(),
      initial = source();
    const old = new OffWikiDraft(owner, initial, persisted, () => {});
    edit(old.doc, " kept private");
    old.retire();
    const current = new OffWikiDraft(owner, { ...initial, writable: false }, persisted, () => {});
    expect(current.start.writable).toBe(false);
    expect(JSON.stringify(current.mine)).toContain("kept private");
    expect(
      await current.save(
        () =>
          new Promise<BodySaveResult>(() => {
            throw new Error("read-only owner attempted a write");
          }),
      ),
    ).toBe(false);
    current.retire();
  });
  test("edits preserve native history and stable IDs, references and marks", async () => {
    const initial = source("😀 ссылка");
    const draft = new OffWikiDraft(
      owner,
      initial,
      storage(),
      () => {},
      () => commandId,
    );
    edit(draft.doc, " mine");
    let sent!: BodySaveCommand;
    expect(
      await draft.save(
        (command) =>
          new Promise<BodySaveResult>((resolve) => {
            sent = command;
            resolve(result(command));
            return;
          }),
      ),
    ).toBe(true);
    const fresh = loadBody(initial, targetId);
    Y.applyUpdate(
      fresh,
      Uint8Array.from(atob(sent.updateV1), (byte) => byte.charCodeAt(0)),
    );
    expect(yDocToTiptapJson(fresh)).toEqual(draft.mine);
    expect(JSON.stringify(draft.mine)).toContain("kept-block");
    expect(JSON.stringify(draft.mine)).toContain("https://example.com");
    expect(JSON.stringify(draft.mine)).toContain("bold");
    expect(draft.start.tailSeq).toBe("9007199254740994");
    fresh.destroy();
    draft.retire();
  });
  test("lost response retries exact command bytes despite later edits", async () => {
    const draft = new OffWikiDraft(
      owner,
      source(),
      storage(),
      () => {},
      () => commandId,
    );
    edit(draft.doc, " first");
    let first!: BodySaveCommand;
    expect(
      draft.save(
        (command) =>
          new Promise<BodySaveResult>(() => {
            first = structuredClone(command);
            throw new Error("connection lost after commit");
          }),
      ),
    ).rejects.toThrow();
    await Promise.resolve();
    edit(draft.doc, " later");
    expect(
      await draft.save(
        (command) =>
          new Promise<BodySaveResult>((resolve) => {
            expect(command).toEqual(first);
            resolve(result(command));
            return;
          }),
      ),
    ).toBe(false);
    expect(draft.dirty).toBe(true);
    expect(JSON.stringify(draft.mine)).toContain("later");
    expect(JSON.stringify(draft.start.contentJson)).not.toContain("later");
    draft.retire();
  });
  test("edits during await stay dirty after the saved prefix is confirmed", async () => {
    let next = 0;
    const draft = new OffWikiDraft(
      owner,
      source(),
      storage(),
      () => {},
      () => `${commandId.slice(0, -1)}${String(++next)}`,
    );
    edit(draft.doc, " first");
    const pending = deferred<BodySaveResult>();
    let first!: BodySaveCommand;
    const save = draft.save((command) => {
      first = command;
      return pending.promise;
    });
    edit(draft.doc, " second");
    pending.resolve(result(first));
    expect(await save).toBe(false);
    expect(
      await draft.save(
        (command) =>
          new Promise<BodySaveResult>((resolve) => {
            expect(command.commandId).not.toBe(first.commandId);
            expect(command.expectedTailSeq).toBe("9007199254740994");
            resolve(result(command));
            return;
          }),
      ),
    ).toBe(true);
    draft.retire();
  });
  for (const field of ["commandId", "targetId", "tailSeq", "revisionId"] as const) {
    test(`wrong ${field} cannot mark a draft committed`, async () => {
      const draft = new OffWikiDraft(
        owner,
        source(),
        storage(),
        () => {},
        () => commandId,
      );
      edit(draft.doc, " mine");
      expect(
        draft.save(
          (command) =>
            new Promise<BodySaveResult>((resolve) => {
              resolve({
                ...result(command),
                [field]: field === "tailSeq" ? "7" : field === "revisionId" ? "" : "wrong",
              });
            }),
        ),
      ).rejects.toThrow();
      await Promise.resolve();
      expect(draft.durable).toBe(false);
      expect(draft.dirty).toBe(true);
      expect(draft.frozen?.command.commandId).toBe(commandId);
      draft.retire();
    });
  }
  test("retired lifetime never applies a late ACK to a new A owner", async () => {
    const persisted = storage();
    const initial = source();
    const first = new OffWikiDraft(
      owner,
      initial,
      persisted,
      () => {},
      () => commandId,
    );
    edit(first.doc, " owned");
    const pending = deferred<BodySaveResult>();
    let command!: BodySaveCommand;
    const oldSave = first.save((input) => {
      command = input;
      return pending.promise;
    });
    first.retire();
    const other = new OffWikiDraft({ ...owner, actorId: "B" }, initial, persisted, () => {});
    expect(other.dirty).toBe(false);
    other.retire();
    const returned = new OffWikiDraft(owner, initial, persisted, () => {});
    pending.resolve(result(command));
    expect(await oldSave).toBe(false);
    expect(returned.start.tailSeq).toBe(initial.tailSeq);
    expect(returned.dirty).toBe(true);
    expect(
      await returned.save(
        (retry) =>
          new Promise<BodySaveResult>((resolve) => {
            expect(retry).toEqual(command);
            resolve(result(retry));
            return;
          }),
      ),
    ).toBe(true);
    returned.retire();
  });
  test("fresh authorized reload preserves a draft and exposes a conflict without applying latest", () => {
    const persisted = storage();
    const initial = source();
    const draft = new OffWikiDraft(owner, initial, persisted, () => {});
    edit(draft.doc, " my unsaved work");
    const mine = draft.mine;
    draft.retire();
    const current = source("another writer", "9007199254740994");
    const reload = new OffWikiDraft(owner, current, persisted, () => {});
    expect(reload.mine).toEqual(mine);
    expect(reload.latest).toEqual(current);
    expect(reload.comparison?.start).toEqual(initial.contentJson);
    expect(reload.comparison?.mine).toEqual(mine);
    expect(reload.comparison?.current).toEqual(current.contentJson);
    reload.editCurrent();
    expect(reload.mine).toEqual(current.contentJson);
    expect(reload.comparison?.mine).toEqual(mine);
    reload.retire();
    const reopened = new OffWikiDraft(owner, current, persisted, () => {});
    expect(reopened.comparison?.mine).toEqual(mine);
    reopened.retire();
  });
  test("separate tab storage and credential/target slots do not expose another draft", () => {
    const one = storage(),
      two = storage(),
      initial = source();
    const first = new OffWikiDraft(owner, initial, one, () => {});
    edit(first.doc, " private");
    first.retire();
    for (const [scope, store] of [
      [owner, two],
      [{ ...owner, credentialId: "other" }, one],
    ] as const) {
      const second = new OffWikiDraft(scope, initial, store, () => {});
      expect(JSON.stringify(second.mine)).not.toContain("private");
      second.retire();
    }
    expect(ownerKey(owner)).not.toBe(ownerKey({ ...owner, targetId: "other" }));
  });
  test("storage refusal leaves the live owned draft intact and reports the failure", () => {
    const broken = {
      getItem: () => null,
      removeItem: () => {},
      setItem: () => {
        throw new Error("quota");
      },
    };
    const draft = new OffWikiDraft(owner, source(), broken, () => {});
    edit(draft.doc, " retained");
    expect(draft.storageError).toBeInstanceOf(Error);
    expect(JSON.stringify(draft.mine)).toContain("retained");
    draft.retire();
  });
  test("unapplied Markdown survives reload with its exact native base and cannot claim a durable body", () => {
    const persisted = storage(),
      initial = source();
    const draft = new OffWikiDraft(owner, initial, persisted, () => {});
    const base = Y.encodeStateAsUpdate(draft.doc);
    draft.setSourceBuffer({ text: "**still unsaved Markdown**", baseV1: base });
    expect(draft.durable).toBe(false);
    draft.retire();
    const fresh = new OffWikiDraft(owner, initial, persisted, () => {});
    expect(fresh.sourceBuffer).toEqual({
      text: "**still unsaved Markdown**",
      baseV1: encodeUpdate(base),
    });
    expect(fresh.mine).toEqual(initial.contentJson);
    expect(fresh.durable).toBe(false);
    fresh.setSourceBuffer(null);
    expect(fresh.durable).toBe(true);
    fresh.retire();
  });
  test("malformed, mismatched and incomplete native history is refused", () => {
    expect(() => loadBody({ ...source(), targetId: "other" }, targetId)).toThrow();
    expect(() => loadBody({ ...source(), tailSeq: "01" }, targetId)).toThrow();
    expect(() => loadBody({ ...source(), snapshotV1: "broken" }, targetId)).toThrow();
    const doc = loadBody(source(), targetId);
    const vector = Y.encodeStateVector(doc);
    edit(doc, " depends on start");
    const dependent = encodeUpdate(Y.encodeStateAsUpdate(doc, vector));
    doc.destroy();
    expect(() => loadBody({ ...source(), snapshotV1: "", tailV1: [dependent] }, targetId)).toThrow(
      "Incomplete",
    );
  });
  test("no edit means no write, while version exhaustion never wraps", async () => {
    const draft = new OffWikiDraft(owner, source(), storage(), () => {});
    expect(
      await draft.save(
        () =>
          new Promise<BodySaveResult>(() => {
            throw new Error("unexpected write");
          }),
      ),
    ).toBe(true);
    draft.retire();
    const exhausted = new OffWikiDraft(
      owner,
      source("end", "9223372036854775807"),
      storage(),
      () => {},
    );
    edit(exhausted.doc, " extra");
    expect(
      exhausted.save(
        () =>
          new Promise<BodySaveResult>(() => {
            throw new Error("unexpected write");
          }),
      ),
    ).rejects.toThrow("exhausted");
    await Promise.resolve();
    exhausted.retire();
  });
});
