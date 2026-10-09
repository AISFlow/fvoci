import { describe, expect, test } from "bun:test";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { FVOCI_YDOC_FRAGMENT } from "@fvoci/editor/collab";
import { OffWikiDraft, encodeUpdate, ownerKey } from "./off-wiki-draft";
import type { OffDraftCreateBody, OffDraftCreateResponse } from "./document-api";
import type { BodySaveResult } from "./versioned-body-api";

const sourceId = "11111111-1111-4111-8111-111111111111";
const resultId = "22222222-2222-4222-8222-222222222222";
const revisionId = "33333333-3333-4333-8333-333333333333";
const owner = {
  actorId: "actor",
  credentialId: "credential",
  workspaceId: "workspace",
  targetId: sourceId,
};
const destination = { projectId: null, parentId: null, title: "mine copy" };
function harness() {
  const slots = new Map<string, string>();
  const storage = {
    getItem: (key: string) => slots.get(key) ?? null,
    setItem: (key: string, value: string) => {
      slots.set(key, value);
    },
    removeItem: (key: string) => {
      slots.delete(key);
    },
  };
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "original-block" },
        content: [{ type: "text", text: "private mine", marks: [{ type: "bold" }] }],
      },
    ],
  });
  const source = {
    targetId: sourceId,
    tailSeq: "9",
    snapshotV1: encodeUpdate(Y.encodeStateAsUpdate(doc)),
    tailV1: [],
    contentJson: yDocToTiptapJson(doc),
    writable: true,
  };
  doc.destroy();
  return { slots, storage, source, draft: new OffWikiDraft(owner, source, storage, () => {}) };
}
function ack(body: OffDraftCreateBody): OffDraftCreateResponse {
  return {
    commandId: body.commandId,
    tailSeq: "1",
    revisionId,
    document: { id: resultId, workspaceId: owner.workspaceId, parentId: null, projectId: null },
  } as OffDraftCreateResponse;
}

describe("OFF separate-document logical command and private mine", () => {
  test("unconfirmed copy keeps its exact retry while manual original resolution and new saves progress", async () => {
    const h = harness();
    let copy!: OffDraftCreateBody;
    expect(
      h.draft.createDistinct(
        destination,
        (body) =>
          new Promise<OffDraftCreateResponse>(() => {
            copy = structuredClone(body);
            throw new Error("copy response lost");
          }),
      ),
    ).rejects.toThrow("copy response lost");
    await Promise.resolve();
    const privateMine = encodeUpdate(Y.encodeStateAsUpdate(h.draft.doc));
    const currentDoc = tiptapJsonToYDoc({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "current-block" },
          content: [{ type: "text", text: "server current" }],
        },
      ],
    });
    h.draft.conflict({
      ...h.source,
      tailSeq: "10",
      snapshotV1: encodeUpdate(Y.encodeStateAsUpdate(currentDoc)),
      contentJson: yDocToTiptapJson(currentDoc),
    });
    currentDoc.destroy();
    h.draft.editCurrent();
    expect(h.draft.start.tailSeq).toBe("10");
    expect(h.draft.distinct?.body).toEqual(copy);
    expect(h.draft.conflictBackup?.mine).toBe(privateMine);
    const paragraph = h.draft.doc.getXmlFragment(FVOCI_YDOC_FRAGMENT).get(0) as Y.XmlElement;
    (paragraph.get(0) as Y.XmlText).insert(0, "manually kept ");
    let originalWrites = 0;
    expect(
      await h.draft.save(
        (command) =>
          new Promise<BodySaveResult>((resolve) => {
            originalWrites++;
            expect(command.expectedTailSeq).toBe("10");
            expect(command.commandId).not.toBe(copy.commandId);
            resolve({
              commandId: command.commandId,
              targetId: sourceId,
              tailSeq: "11",
              revisionId,
            });
            return;
          }),
      ),
    ).toBe(false); // The independent copy remains unconfirmed, so the whole draft is not durable.
    expect(originalWrites).toBe(1);
    expect(h.draft.start.tailSeq).toBe("11");
    expect(h.draft.dirty).toBe(false);
    expect(h.draft.frozen).toBeNull();
    const savedOriginal = encodeUpdate(Y.encodeStateAsUpdate(h.draft.doc));
    h.draft.setSourceBuffer({
      text: "# still unapplied",
      baseV1: Y.encodeStateAsUpdate(h.draft.doc),
    });
    expect(
      (
        await h.draft.createDistinct(
          { ...destination, title: "ignored new form" },
          (body) =>
            new Promise<OffDraftCreateResponse>((resolve) => {
              expect(body).toEqual(copy);
              resolve(ack(body));
              return;
            }),
        )
      )?.document.id,
    ).toBe(resultId);
    expect(h.draft.start.tailSeq).toBe("11");
    expect(encodeUpdate(Y.encodeStateAsUpdate(h.draft.doc))).toBe(savedOriginal);
    expect(h.draft.sourceBuffer?.text).toBe("# still unapplied");
    h.draft.retire();
  });
  test("unknown result freezes one body/IDs across edits and restart, then keeps original history", async () => {
    const h = harness();
    const sent: OffDraftCreateBody[] = [];
    const send = (body: OffDraftCreateBody) =>
      new Promise<OffDraftCreateResponse>(() => {
        sent.push(structuredClone(body));
        throw new Error("response lost");
      });
    expect(h.draft.createDistinct(destination, send)).rejects.toThrow("response lost");
    await Promise.resolve();
    const paragraph = h.draft.doc.getXmlFragment(FVOCI_YDOC_FRAGMENT).get(0) as Y.XmlElement;
    (paragraph.get(0) as Y.XmlText).insert(0, "later edit ");
    const original = encodeUpdate(Y.encodeStateAsUpdate(h.draft.doc));
    h.draft.retire();
    const resumed = new OffWikiDraft(owner, h.source, h.storage, () => {});
    const copied = await resumed.createDistinct(
      { ...destination, title: "changed form must not change retry" },
      (body) =>
        new Promise<OffDraftCreateResponse>((resolve) => {
          sent.push(structuredClone(body));
          resolve(ack(body));
          return;
        }),
    );
    expect(sent[1]).toEqual(sent[0]);
    expect(JSON.stringify(sent[0].contentJson)).not.toContain("original-block");
    expect(JSON.stringify(sent[0].contentJson)).not.toContain("later edit");
    expect(encodeUpdate(Y.encodeStateAsUpdate(resumed.doc))).toBe(original);
    expect(resumed.start.tailSeq).toBe("9");
    expect(resumed.dirty).toBe(true);
    expect(copied?.document.id).toBe(resultId);
    expect(resumed.distinct).toBeNull();
    expect(h.slots.has(ownerKey(owner))).toBe(true);
    resumed.retire();
  });
  test("single flight and retirement cannot apply an old result, including remount ABA", async () => {
    const h = harness();
    let resolve!: (value: OffDraftCreateResponse) => void;
    let captured!: OffDraftCreateBody;
    let requests = 0;
    const pending = h.draft.createDistinct(destination, async (body) => {
      captured = body;
      requests++;
      return new Promise((done) => {
        resolve = done;
      });
    });
    expect(
      await h.draft.createDistinct(
        destination,
        () =>
          new Promise<OffDraftCreateResponse>(() => {
            throw new Error("double create");
          }),
      ),
    ).toBeNull();
    expect(requests).toBe(1);
    h.draft.retire();
    const remounted = new OffWikiDraft(owner, h.source, h.storage, () => {});
    resolve(ack(captured));
    expect(await pending).toBeNull();
    expect(remounted.distinct?.body.commandId).toBe(captured.commandId);
    remounted.retire();
  });
  test("wrong command/new-document/version/revision acknowledgment retains exact retry", async () => {
    for (const change of [
      "command",
      "target",
      "version",
      "revision",
      "workspace",
      "project",
      "parent",
    ]) {
      const h = harness();
      expect(
        h.draft.createDistinct(
          destination,
          (body) =>
            new Promise<OffDraftCreateResponse>((resolve) => {
              const result = ack(body);
              if (change === "command") result.commandId = crypto.randomUUID();
              if (change === "target") result.document.id = sourceId;
              if (change === "version") result.tailSeq = "0";
              if (change === "revision") result.revisionId = "";
              if (change === "workspace") result.document.workspaceId = "another-workspace";
              if (change === "project")
                result.document.projectId = "44444444-4444-4444-8444-444444444444";
              if (change === "parent")
                result.document.parentId = "55555555-5555-4555-8555-555555555555";
              resolve(result);
              return;
            }),
        ),
      ).rejects.toThrow("Unmatched distinct");
      await Promise.resolve();
      expect(h.draft.distinct).not.toBeNull();
      expect(h.draft.start.tailSeq).toBe("9");
      const recovered = await h.draft.createDistinct(
        destination,
        (body) =>
          new Promise<OffDraftCreateResponse>((resolve) => {
            resolve(ack(body));
          }),
      );
      expect(recovered?.document.id).toBe(resultId);
      h.draft.retire();
    }
  });
  test("invalid form refuses before freezing or transport and a corrected logical request still progresses", async () => {
    const h = harness();
    let requests = 0;
    const send = (body: OffDraftCreateBody) =>
      new Promise<OffDraftCreateResponse>((resolve) => {
        requests++;
        resolve(ack(body));
        return;
      });
    for (const title of ["   ", "x".repeat(301)]) {
      expect(h.draft.createDistinct({ ...destination, title }, send)).rejects.toThrow(
        "Invalid document",
      );
      await Promise.resolve();
      expect(h.draft.distinct).toBeNull();
      expect(requests).toBe(0);
    }
    const copied = await h.draft.createDistinct({ ...destination, title: " corrected " }, send);
    expect(requests).toBe(1);
    expect(copied?.document.id).toBe(resultId);
    expect(h.draft.start.tailSeq).toBe("9");
    h.draft.retire();
  });
  test("separate confirmed logical copies choose fresh command and block IDs without altering mine", async () => {
    const h = harness();
    const sent: OffDraftCreateBody[] = [];
    const send = (body: OffDraftCreateBody) =>
      new Promise<OffDraftCreateResponse>((resolve) => {
        sent.push(structuredClone(body));
        resolve(ack(body));
        return;
      });
    await h.draft.createDistinct(destination, send);
    await h.draft.createDistinct(destination, send);
    expect(sent).toHaveLength(2);
    expect(sent[0].commandId).not.toBe(sent[1].commandId);
    expect(sent[0].contentJson).not.toEqual(sent[1].contentJson);
    expect(h.draft.mine).toEqual(h.source.contentJson);
    expect(h.draft.start.tailSeq).toBe("9");
    h.draft.retire();
  });
});
