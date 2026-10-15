// Maintained selected ON native-history oracle, extracted without changing its assertions.
import { expect, type Page } from "@playwright/test";
import * as Y from "yjs";
import { MessageType } from "@hocuspocus/provider";
import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as sync from "y-protocols/sync";
import { yDocToTiptapJson } from "../../../packages/editor/src/collab-tiptap";
import { emojiGlyph } from "../../../packages/editor/src/emoji-glyph";
import { extractText, walkTiptap, type TiptapWalkNode } from "../../../packages/editor/src/extract";
import type { components } from "../src/generated/api";
import { decodeHocuspocusFrame, frameBytes, UUID_RE } from "./collab-wire";
import { createHash } from "node:crypto";
import type { Manifest, LocalLease } from "../../../tools/turso/ui-record";
import type { Audit } from "../../../tools/turso/ui-audit";
import { isDeepStrictEqual } from "node:util";
type RevisionDetail = components["schemas"]["RevisionDetailResponse"];

export interface SelectedSyncWrite {
  workspaceId: string;
  documentId: string;
  actorId: string;
  sha256: string;
  length: number;
  empty: boolean;
  applied: boolean;
}

/** Safe projection of real sync traffic. Installed lib0/Yjs/Hocuspocus codecs
 * own the protocol. Neither authentication frames nor credentials are retained. */
export function observeSelectedWrites(page: Page, actorId: string) {
  const writes: SelectedSyncWrite[] = [];
  const syncTypes: readonly number[] = [MessageType.Sync];
  const statusTypes: readonly number[] = [MessageType.SyncStatus];
  let invalid = false;
  page.on("websocket", (socket) => {
    if (!socket.url().includes("/collab")) return;
    const pending: SelectedSyncWrite[] = [];
    socket.on("framesent", ({ payload }) => {
      const frame = decodeHocuspocusFrame(frameBytes(payload));
      if (frame?.kind !== "other" || !syncTypes.includes(frame.type)) return;
      try {
        const decoder = decoding.createDecoder(frameBytes(payload));
        const room = decoding.readVarString(decoder);
        decoding.readVarUint(decoder);
        const step = decoding.readVarUint(decoder);
        if (step !== sync.messageYjsSyncStep2 && step !== sync.messageYjsUpdate) return;
        const [workspaceId, kind, documentId, extra] = room.split(":");
        if (
          !workspaceId ||
          !documentId ||
          extra ||
          kind !== "document" ||
          !UUID_RE.test(workspaceId) ||
          !UUID_RE.test(documentId)
        )
          throw Error();
        const bytes = decoding.readVarUint8Array(decoder);
        if (decoding.hasContent(decoder) || bytes.length === 0 || writes.length >= 10000)
          throw Error();
        const row = {
          workspaceId,
          documentId,
          actorId,
          sha256: createHash("sha256").update(bytes).digest("hex"),
          length: bytes.length,
          empty: bytes.length === 2 && bytes[0] === 0 && bytes[1] === 0,
          applied: false,
        };
        writes.push(row);
        pending.push(row);
      } catch {
        invalid = true;
      }
    });
    socket.on("framereceived", ({ payload }) => {
      const frame = decodeHocuspocusFrame(frameBytes(payload));
      if (frame?.kind !== "other" || !statusTypes.includes(frame.type)) return;
      try {
        const decoder = decoding.createDecoder(frameBytes(payload));
        const room = decoding.readVarString(decoder);
        decoding.readVarUint(decoder);
        const applied = decoding.readVarInt(decoder);
        const row = pending.shift();
        if (
          !row ||
          room !== `${row.workspaceId}:document:${row.documentId}` ||
          applied !== 1 ||
          decoding.hasContent(decoder)
        )
          throw Error();
        row.applied = true;
      } catch {
        invalid = true;
      }
    });
  });
  return () => {
    if (invalid || writes.some((row) => !row.applied))
      throw Error("SELECTED_FINAL_SYNC_UNCONFIRMED");
    return structuredClone(writes);
  };
}

export interface SelectedNativeReceipt {
  seq: number;
  op: string;
  actor: string;
  sha: string;
  len: number;
}

/** The parent passes only new receipts for this exact target after proving
 * original rows retained. Every nonempty successful wire write is consumed
 * once; an additional denied/foreign/duplicate native operation fails closed. */
export function assertSelectedSuccessfulWrites(
  receipts: SelectedNativeReceipt[],
  writes: SelectedSyncWrite[],
  workspaceId: string,
  documentId: string,
): void {
  const successful = writes.filter(
    (w) => w.workspaceId === workspaceId && w.documentId === documentId,
  );
  if (
    successful.some(
      (w) =>
        !w.applied ||
        !UUID_RE.test(w.actorId) ||
        !/^[a-f0-9]{64}$/.test(w.sha256) ||
        !Number.isSafeInteger(w.length) ||
        w.length < 1 ||
        (w.empty &&
          (w.length !== 2 ||
            w.sha256 !==
              createHash("sha256")
                .update(new Uint8Array([0, 0]))
                .digest("hex"))),
    ) ||
    new Set(receipts.map((r) => r.op)).size !== receipts.length ||
    new Set(receipts.map((r) => r.seq)).size !== receipts.length
  )
    throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
  const remaining = successful.filter((w) => !w.empty);
  for (const receipt of receipts) {
    if (!Number.isSafeInteger(receipt.seq) || receipt.seq < 1 || !UUID_RE.test(receipt.op))
      throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
    const index = remaining.findIndex(
      (w) => w.actorId === receipt.actor && w.sha256 === receipt.sha && w.length === receipt.len,
    );
    if (index < 0) throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
    remaining.splice(index, 1);
  }
  if (remaining.length) throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
}

/** PARENT ONLY. Browser actors never call this function or read its environment.
 * The existing admitted parent supplies its manifest, exact target and private
 * environment, and retains ownership/preservation/event auditing of the phase. */
export async function observeSelectedFinalNative(
  manifest: Manifest,
  environment: Record<string, string>,
  expected: {
    source: string;
    tree: string;
    targetSha256: string;
    capsule: { path: string; sha256: string };
  },
  workspaceId: string,
  documentIds: string[],
  lease?: LocalLease,
) {
  if (
    process.env.TEST_WORKER_INDEX !== undefined ||
    process.env.PLAYWRIGHT_JSON_OUTPUT_FILE !== undefined
  )
    throw Error("SELECTED_FINAL_PARENT_ONLY");
  const { currentBuild } = await import("../../../tools/turso/ui-record");
  const { fixture } = await import("../../../tools/turso/ui-native");
  const { withProcesses } = await import("../../../tools/turso/ui-processes");
  const { privateRead } = await import("../../../tools/turso/ui-common");
  const { sha } = await import("../../../tools/selected-backend-ci/io");
  const check = () => {
    if (!environment.FVOCI_LIBSQL_URL || !environment.FVOCI_LIBSQL_AUTH_TOKEN)
      throw Error("SELECTED_FINAL_NATIVE_BINDING_REFUSED");
    const current = currentBuild(lease);
    const capsule = privateRead(expected.capsule.path, 16 * 1024 * 1024) as {
      manifest: unknown;
      environment: unknown;
    };
    if (
      sha(expected.capsule.path) !== expected.capsule.sha256 ||
      !isDeepStrictEqual(capsule.manifest, manifest) ||
      !isDeepStrictEqual(capsule.environment, environment) ||
      !isDeepStrictEqual(current, manifest) ||
      current.sourceInputs.source !== expected.source ||
      current.sourceInputs.tree !== expected.tree ||
      createHash("sha256").update(environment.FVOCI_LIBSQL_URL).digest("hex") !==
        expected.targetSha256 ||
      !UUID_RE.test(workspaceId) ||
      documentIds.length === 0 ||
      new Set(documentIds).size !== documentIds.length ||
      documentIds.some((id) => !UUID_RE.test(id))
    )
      throw Error("SELECTED_FINAL_NATIVE_BINDING_REFUSED");
  };
  check();
  const observation = await withProcesses((scope) =>
    fixture(scope, manifest, "observe", environment, { workspaceId, documentIds }),
  );
  check();
  if (
    observation.backend !== "libsql-remote" ||
    observation.workspace_id !== workspaceId ||
    !observation.rows ||
    typeof observation.rows !== "object" ||
    Array.isArray(observation.rows) ||
    !isDeepStrictEqual(Object.keys(observation.rows).sort(), [...documentIds].sort())
  )
    throw Error("SELECTED_FINAL_NATIVE_BINDING_REFUSED");
  return observation;
}

/** Parent's existing complete-primary preservation audit remains mandatory.
 * It owns the exact allowed product/member event delta and operational rows;
 * this adapter adds one-to-one successful native write/receipt qualification. */
export async function auditSelectedFinalNative(
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  audit: Audit,
  targets: { workspaceId: string; documentId: string; receipts: SelectedNativeReceipt[] }[],
  writes: SelectedSyncWrite[],
) {
  const { assertPreserved } = await import("../../../tools/turso/ui-audit");
  assertPreserved(before, after, audit);
  const keys = targets.map((t) => `${t.workspaceId}:document:${t.documentId}`);
  if (
    new Set(keys).size !== keys.length ||
    writes.some((w) => !keys.includes(`${w.workspaceId}:document:${w.documentId}`))
  )
    throw Error("SELECTED_FINAL_NATIVE_DELTA_REFUSED");
  for (const target of targets)
    assertSelectedSuccessfulWrites(target.receipts, writes, target.workspaceId, target.documentId);
}

export function documentNodeIds(value: unknown): string[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const node = value as { attrs?: { id?: unknown }; content?: unknown[] };
  const own = typeof node.attrs?.id === "string" ? [node.attrs.id] : [];
  return [...own, ...(node.content ?? []).flatMap(documentNodeIds)];
}

export function expectCanonicalContent(
  content: unknown,
  retained: string,
  removed: string,
  expectedIds: string[],
): void {
  const text = extractText(content);
  expect(text).toBe(`${retained} 새 편집`);
  expect(text).not.toContain(removed);
  const emojis: TiptapWalkNode[] = [];
  walkTiptap(content, (node) => {
    if (node.type === "emoji") emojis.push(node);
  });
  expect(emojis).toHaveLength(1);
  const emoji = emojis[0];
  if (!emoji) throw Error("SELECTED_FINAL_EMOJI_MISSING");
  expect(emoji.attrs?.name).toBe("grinning");
  expect(emojiGlyph(emoji)).toBe("😀");
  const ids = documentNodeIds(content);
  expect(ids.length).toBeGreaterThan(0);
  for (const id of ids) expect(id).toMatch(UUID_RE);
  expect(ids).toEqual(expectedIds);
}

export function expectCanonicalOracleControls(
  content: unknown,
  retained: string,
  removed: string,
  ids: string[],
): string[] {
  // Mutate read-only JSON copies using the maintained walker. These controls
  // never touch a live editor/Y.Doc, native bytes, or server state.
  const controls: [string, (copy: unknown) => void][] = [
    [
      "emoji absent",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (node.type === "emoji") {
            node.type = "text";
            node.text = "";
            delete node.attrs;
          }
        });
      },
    ],
    [
      "wrong emoji name despite matching explicit glyph",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (node.type === "emoji")
            node.attrs = { ...node.attrs, name: "wrong-name", emoji: "😀" };
        });
      },
    ],
    [
      "emoji misplaced",
      (copy) => {
        let emoji: TiptapWalkNode | undefined;
        walkTiptap(copy, (node) => {
          if (node.type === "emoji") emoji = node;
        });
        walkTiptap(copy, (node) => {
          if (emoji && node.content?.includes(emoji))
            node.content = [emoji, ...node.content.filter((child) => child !== emoji)];
        });
      },
    ],
    [
      "retained text missing",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (typeof node.text === "string") node.text = node.text.replace("동일한", "");
        });
      },
    ],
    [
      "deleted text survives",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (typeof node.text === "string" && node.text.includes("새 편집")) node.text += removed;
        });
      },
    ],
    [
      "block identity changed",
      (copy) => {
        walkTiptap(copy, (node) => {
          if (node.type === "paragraph")
            node.attrs = { ...node.attrs, id: "00000000-0000-4000-8000-000000000340" };
        });
      },
    ],
  ];
  expectCanonicalContent(content, retained, removed, ids);
  for (const [label, mutate] of controls) {
    const copy = structuredClone(content);
    mutate(copy);
    expect(() => {
      expectCanonicalContent(copy, retained, removed, ids);
    }, label).toThrow();
  }
  return controls.map(([label]) => label);
}

export type NativeWireFrame = { direction: "sent" | "received"; room: string; bytes: Uint8Array };

export function observeNativeWire(page: Page): NativeWireFrame[] {
  const frames: NativeWireFrame[] = [];
  const syncTypes: readonly number[] = [MessageType.Sync];
  page.on("websocket", (socket) => {
    if (!socket.url().includes("/collab")) return;
    const observe = (direction: "sent" | "received", payload: string | Buffer) => {
      const bytes = frameBytes(payload);
      const frame = decodeHocuspocusFrame(bytes);
      if (frame?.kind === "other" && syncTypes.includes(frame.type))
        frames.push({ direction, room: frame.routingKey, bytes: Uint8Array.from(bytes) });
    };
    socket.on("framesent", (frame) => {
      observe("sent", frame.payload);
    });
    socket.on("framereceived", (frame) => {
      observe("received", frame.payload);
    });
  });
  return frames;
}

export function nativeHistory(
  frames: NativeWireFrame[],
  room: string,
  receivedOnly: boolean,
): Y.Doc {
  const native = new Y.Doc({ gc: false });
  try {
    let receivedStep2 = 0;
    for (const frame of frames) {
      if (frame.room !== room || (receivedOnly && frame.direction !== "received")) continue;
      // Reuse the existing Hocuspocus frame filter and installed public codecs.
      // The response encoder is a local sink: no reply is sent to any socket.
      const decoder = decoding.createDecoder(frame.bytes);
      expect(decoding.readVarString(decoder)).toBe(room);
      expect(decoding.readVarUint(decoder)).toBe(MessageType.Sync);
      const kind = sync.readSyncMessage(
        decoder,
        encoding.createEncoder(),
        native,
        null,
        (error) => {
          throw error;
        },
      );
      expect(decoder.pos).toBe(frame.bytes.length);
      if (frame.direction === "received" && kind === sync.messageYjsSyncStep2) receivedStep2++;
    }
    expect(receivedStep2, "real server full-sync response is required").toBeGreaterThan(0);
    return native;
  } catch (error) {
    native.destroy();
    throw error;
  }
}

export function expectRetainedNative(
  detail: RevisionDetail,
  native: Y.Doc,
  retained: string,
  removed: string,
): void {
  // Revision ySnapshot is a DSSV snapshot, not a full update. Its full retained
  // native history comes only from observed real sync frames (gc=false).
  const snapshot = Y.decodeSnapshot(Uint8Array.from(Buffer.from(detail.ySnapshot, "base64")));
  expect(snapshot.sv.size).toBeGreaterThan(0);
  expect(snapshot.ds.clients.size, "revision snapshot retains the deletion set").toBeGreaterThan(0);
  expect(
    Y.equalSnapshots(Y.snapshot(native), snapshot),
    "revision matches observed native state",
  ).toBe(true);
  const update = Y.encodeStateAsUpdate(native);
  expect(update.byteLength).toBeGreaterThan(2);
  const decoded = Y.decodeUpdate(update);
  expect(decoded.ds.clients.size, "full native history retains the deletion set").toBeGreaterThan(
    0,
  );
  const retainedStrings = decoded.structs.flatMap((struct) =>
    struct instanceof Y.Item && struct.content instanceof Y.ContentString
      ? [struct.content.str]
      : [],
  );
  expect(retainedStrings.join(""), "deleted source bytes survive the native history").toContain(
    removed,
  );
  const xml: unknown = native.getXmlFragment("prosemirror").toJSON();
  if (typeof xml !== "string") throw new Error("native XML serialization must be a string");
  expect([...native.share.keys()]).toEqual(["prosemirror"]);
  const ids = documentNodeIds(detail.contentJson);
  expectCanonicalContent(yDocToTiptapJson(native), retained, removed, ids);
  expect(xml).not.toContain(removed);
  for (const id of ids) expect(xml).toContain(id);
  const historical = new Y.Doc({ gc: false });
  try {
    Y.createDocFromSnapshot(native, snapshot, historical);
    const content = yDocToTiptapJson(historical);
    expect(content).toEqual(detail.contentJson);
    expectCanonicalContent(content, retained, removed, ids);
  } finally {
    historical.destroy();
  }
}

export function expectNativeHistoryControls(
  detail: RevisionDetail,
  native: Y.Doc,
  retained: string,
  removed: string,
): string[] {
  expectRetainedNative(detail, native, retained, removed);
  const snapshot = Y.decodeSnapshot(Buffer.from(detail.ySnapshot, "base64"));
  const wrongVector = new Map(snapshot.sv);
  const first = wrongVector.entries().next().value;
  if (!first) throw new Error("nonempty native snapshot state vector is required");
  wrongVector.set(first[0], first[1] + 1);
  const wrongSnapshot = Y.encodeSnapshot(Y.createSnapshot(snapshot.ds, wrongVector));
  expect(() => {
    expectRetainedNative(
      { ...detail, ySnapshot: Buffer.from(wrongSnapshot).toString("base64") },
      native,
      retained,
      removed,
    );
  }, "a valid but wrong revision snapshot must fail").toThrow();
  const missingHistory = new Y.Doc({ gc: true });
  try {
    // A native-byte observer copy only, never a live editor or JSON reseed.
    Y.applyUpdate(missingHistory, Y.encodeStateAsUpdate(native));
    missingHistory.gc = false;
    expect(Y.equalSnapshots(Y.snapshot(missingHistory), snapshot)).toBe(true);
    expect(yDocToTiptapJson(missingHistory)).toEqual(detail.contentJson);
    expect(() => {
      expectRetainedNative(detail, missingHistory, retained, removed);
    }, "matching text/snapshot without deleted native source bytes must fail").toThrow();
  } finally {
    missingHistory.destroy();
  }
  return ["valid wrong snapshot", "same text and snapshot but missing deleted native source bytes"];
}
