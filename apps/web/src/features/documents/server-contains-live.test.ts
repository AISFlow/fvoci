import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { OffWikiDraft, encodeUpdate, loadBody, serverContainsLive } from "./off-wiki-draft";
import type { VersionedBody } from "./versioned-body-api";

// link-mark-live.v1 is Y.encodeStateAsUpdate of tiptapJsonToYDoc (gc: false) for
// a paragraph id "link-block" whose text "saved link" has one link mark:
// href "https://example.com/path", target "_blank",
// rel "noopener noreferrer nofollow", class null, title null.
// link-mark-yrs-snapshot.v1 is not a yjs re-encode. It is the Snapshot
// update_b64 from crates/collab-engine (vendored yrs 0.28.0, small-client)
// after Load { snapshot_b64: live, tail_b64: [], encoding: 1 } then
// Apply { update_b64: live, encoding: 1 } then Snapshot.
const liveBytes = Uint8Array.from(
  readFileSync(new URL("./fixtures/link-mark-live.v1", import.meta.url)),
);
const serverBytes = Uint8Array.from(
  readFileSync(new URL("./fixtures/link-mark-yrs-snapshot.v1", import.meta.url)),
);
const targetId = "11111111-1111-4111-8111-111111111111";
const owner = {
  actorId: "actor",
  credentialId: "credential",
  workspaceId: "workspace",
  targetId,
};

function docFrom(update: Uint8Array): Y.Doc {
  const doc = new Y.Doc({ gc: false });
  Y.applyUpdate(doc, update);
  if (doc.store.pendingStructs || doc.store.pendingDs) {
    doc.destroy();
    throw new Error("Incomplete update");
  }
  return doc;
}
function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) return false;
  for (let i = 0; i < left.byteLength; i += 1) if (left[i] !== right[i]) return false;
  return true;
}
function stateVectorCovers(server: Y.Doc, live: Y.Doc): boolean {
  const outer = Y.decodeStateVector(Y.encodeStateVector(server));
  const inner = Y.decodeStateVector(Y.encodeStateVector(live));
  for (const [client, clock] of inner) if ((outer.get(client) ?? 0) < clock) return false;
  return true;
}
function versioned(update: Uint8Array, tailSeq: string): VersionedBody {
  const doc = docFrom(update);
  try {
    return {
      targetId,
      tailSeq,
      snapshotV1: encodeUpdate(update),
      tailV1: [],
      contentJson: yDocToTiptapJson(doc),
      writable: true,
    };
  } finally {
    doc.destroy();
  }
}
function storage() {
  const slots = new Map<string, string>();
  return {
    getItem: (key: string) => slots.get(key) ?? null,
    setItem: (key: string, value: string) => {
      slots.set(key, value);
    },
    removeItem: (key: string) => {
      slots.delete(key);
    },
  };
}

describe("server contains live history", () => {
  test("a yrs Load-Apply-Snapshot of a link mark body contains the live document", () => {
    expect(sameBytes(liveBytes, serverBytes)).toBe(false);
    const live = docFrom(liveBytes);
    const server = docFrom(serverBytes);
    try {
      expect(sameBytes(Y.encodeStateAsUpdate(live), Y.encodeStateAsUpdate(server))).toBe(false);
      expect(serverContainsLive(server, liveBytes)).toBe(true);
      expect(serverContainsLive(server, Y.encodeStateAsUpdate(live))).toBe(true);
    } finally {
      live.destroy();
      server.destroy();
    }
  });
  test("an unsaved deletion keeps the state vector and is not contained", () => {
    const server = docFrom(liveBytes);
    const live = docFrom(liveBytes);
    try {
      const text = live.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
      (text.get(0) as Y.XmlText).delete(0, 1);
      expect(stateVectorCovers(server, live)).toBe(true);
      expect(sameBytes(Y.encodeStateVector(server), Y.encodeStateVector(live))).toBe(true);
      expect(serverContainsLive(server, Y.encodeStateAsUpdate(live))).toBe(false);
    } finally {
      server.destroy();
      live.destroy();
    }
  });
  test("a live insert missing from the server is not contained", () => {
    const server = docFrom(liveBytes);
    const live = docFrom(liveBytes);
    try {
      const text = live.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
      (text.get(0) as Y.XmlText).insert((text.get(0) as Y.XmlText).length, " extra");
      expect(stateVectorCovers(server, live)).toBe(false);
      expect(serverContainsLive(server, Y.encodeStateAsUpdate(live))).toBe(false);
    } finally {
      server.destroy();
      live.destroy();
    }
  });
});

describe("observeAuthorizedBody", () => {
  test("a yrs re-encoded link body at the same tail is not a conflict", () => {
    const draft = new OffWikiDraft(owner, versioned(liveBytes, "4"), storage(), () => {});
    try {
      const observed = versioned(serverBytes, "4");
      const reader = loadBody(observed, targetId);
      try {
        expect(encodeUpdate(Y.encodeStateAsUpdate(reader))).not.toBe(
          encodeUpdate(Y.encodeStateAsUpdate(draft.doc)),
        );
      } finally {
        reader.destroy();
      }
      draft.observeAuthorizedBody(observed);
      expect(draft.latest).toBeNull();
      expect(draft.comparison).toBeNull();
      expect(draft.durable).toBe(true);
      expect(draft.start.tailSeq).toBe("4");
    } finally {
      draft.retire();
    }
  });
  test("a different body at the same tail stays a conflict", () => {
    const draft = new OffWikiDraft(owner, versioned(liveBytes, "4"), storage(), () => {});
    const otherDoc = tiptapJsonToYDoc({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "other-block" },
          content: [{ type: "text", text: "different body" }],
        },
      ],
    });
    const otherUpdate = Y.encodeStateAsUpdate(otherDoc);
    otherDoc.destroy();
    try {
      const observed = versioned(otherUpdate, draft.start.tailSeq);
      expect(observed.tailSeq).toBe(draft.start.tailSeq);
      draft.observeAuthorizedBody(observed);
      expect(draft.latest?.snapshotV1).toBe(observed.snapshotV1);
      expect(draft.comparison?.current).toEqual(observed.contentJson);
      expect(draft.comparison?.mine).toEqual(draft.mine);
      expect(draft.durable).toBe(false);
    } finally {
      draft.retire();
    }
  });
});
