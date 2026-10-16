import { expect, test } from "bun:test";
import * as Y from "yjs";
import * as encoding from "lib0/encoding";
import * as sync from "y-protocols/sync";
import {
  nativeHistory,
  assertSelectedSuccessfulWrites,
  observeSelectedWrites,
  type NativeWireFrame,
  type SelectedSyncWrite,
} from "./selected-final-native";
import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { MessageType } from "@hocuspocus/provider";

const room = "00000000-0000-4000-8000-000000000001:document:00000000-0000-4000-8000-000000000002";
function frame(doc: Y.Doc, kind: "full" | "update" = "full"): NativeWireFrame {
  const encoder = encoding.createEncoder();
  encoding.writeVarString(encoder, room);
  encoding.writeVarUint(encoder, 0);
  if (kind === "full") sync.writeSyncStep2(encoder, doc);
  else sync.writeUpdate(encoder, Y.encodeStateAsUpdate(doc));
  return { direction: "received", room, bytes: encoding.toUint8Array(encoder) };
}

test("native readback requires an actual server full-sync, correct room and complete frame", () => {
  const source = new Y.Doc({ gc: false });
  source.getText("native-test").insert(0, "retained native bytes");
  const full = frame(source);
  const result = nativeHistory([full], room, true);
  try {
    expect(result.getText("native-test").toJSON()).toBe("retained native bytes");
    for (const wrong of [
      [],
      [frame(source, "update")],
      [{ ...full, direction: "sent" as const }],
      [{ ...full, room: "foreign-room" }],
      [{ ...full, bytes: full.bytes.slice(0, -1) }],
      [{ ...full, bytes: Uint8Array.from([...full.bytes, 0]) }],
    ])
      expect(() => nativeHistory(wrong, room, true)).toThrow();
  } finally {
    source.destroy();
    result.destroy();
  }
});

test("safe actor projection requires matching real SyncStatus on the owning connection", () => {
  const source = new Y.Doc({ gc: false });
  source.getText("native-test").insert(0, "bytes");
  const full = frame(source);
  const setup = () => {
    let connect: ((socket: unknown) => void) | undefined;
    const handlers = new Map<string, (frame: { payload: Buffer }) => void>();
    const page = {
      on(_event: string, callback: typeof connect) {
        connect = callback;
      },
    } as unknown as Page;
    const read = observeSelectedWrites(page, "00000000-0000-4000-8000-000000000003");
    connect?.({
      url: () => "/collab",
      on(event: string, callback: (frame: { payload: Buffer }) => void) {
        handlers.set(event, callback);
      },
    });
    const status = (routing = room, applied = 1) => {
      const encoder = encoding.createEncoder();
      encoding.writeVarString(encoder, routing);
      encoding.writeVarUint(encoder, MessageType.SyncStatus);
      encoding.writeVarInt(encoder, applied);
      handlers.get("framereceived")?.({ payload: Buffer.from(encoding.toUint8Array(encoder)) });
    };
    const send = () => handlers.get("framesent")?.({ payload: Buffer.from(full.bytes) });
    return { read, send, status };
  };
  try {
    const good = setup();
    good.send();
    expect(() => good.read()).toThrow("SELECTED_FINAL_SYNC_UNCONFIRMED");
    good.status();
    expect(good.read()).toHaveLength(1);
    expect(good.read()[0]?.applied).toBe(true);
    for (const wrong of [
      (s: ReturnType<typeof setup>) => {
        s.status("foreign");
      },
      (s: ReturnType<typeof setup>) => {
        s.status(room, 0);
      },
      (s: ReturnType<typeof setup>) => {
        s.status();
        s.status();
      },
    ]) {
      const bad = setup();
      bad.send();
      wrong(bad);
      expect(() => bad.read()).toThrow("SELECTED_FINAL_SYNC_UNCONFIRMED");
    }
  } finally {
    source.destroy();
  }
});

test("parent native delta refuses an unacknowledged, extra, foreign or duplicate operation", () => {
  const [workspaceId, , documentId] = room.split(":");
  if (!workspaceId || !documentId) throw Error("invalid test target");
  const actorId = "00000000-0000-4000-8000-000000000003";
  const write: SelectedSyncWrite = {
    workspaceId,
    documentId,
    actorId,
    sha256: "a".repeat(64),
    length: 10,
    empty: false,
    applied: true,
  };
  const receipt = {
    seq: 1,
    op: "00000000-0000-4000-8000-000000000004",
    actor: actorId,
    sha: write.sha256,
    len: write.length,
  };
  assertSelectedSuccessfulWrites([receipt], [write], workspaceId, documentId);
  for (const [receipts, writes] of [
    [[receipt], [{ ...write, applied: false }]],
    [[receipt], []],
    [[], [write]],
    [[{ ...receipt, actor: "00000000-0000-4000-8000-000000000005" }], [write]],
    [[receipt, { ...receipt, seq: 2, op: "00000000-0000-4000-8000-000000000006" }], [write]],
    [[receipt], [write, write]],
    [
      [receipt, receipt],
      [write, write],
    ],
    [[receipt], [{ ...write, empty: true }]],
  ] as [(typeof receipt)[], SelectedSyncWrite[]][])
    expect(() => {
      assertSelectedSuccessfulWrites(receipts, writes, workspaceId, documentId);
    }).toThrow();
  assertSelectedSuccessfulWrites(
    [],
    [
      {
        ...write,
        empty: true,
        length: 2,
        sha256: createHash("sha256")
          .update(new Uint8Array([0, 0]))
          .digest("hex"),
      },
    ],
    workspaceId,
    documentId,
  );
});
