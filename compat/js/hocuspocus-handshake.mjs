#!/usr/bin/env node
/**
 * Capture a real @hocuspocus/provider 4.6.0 handshake frame and try to decode
 * it with y-protocols 1.0.7 sync (the "standard Y sync" decoder).
 * A mismatch is a finding, not a Yrs bug: Yrs updateV1 ≠ Hocuspocus framing.
 */
import { writeAuthentication } from "@hocuspocus/common";
import {
  HocuspocusProvider,
  MessageType,
} from "@hocuspocus/provider";
import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as syncProtocol from "y-protocols/sync";
import * as Y from "yjs";

const DOC_NAME = "document:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

function hexHead(u8, n = 32) {
  return Buffer.from(u8.subarray(0, n)).toString("hex");
}

class CaptureWebSocket {
  static frames = [];
  static instances = [];

  constructor(url) {
    this.url = String(url);
    this.readyState = CaptureWebSocket.CONNECTING;
    this.binaryType = "arraybuffer";
    this.bufferedAmount = 0;
    this.protocol = "";
    this.extensions = "";
    this.listeners = { open: [], close: [], message: [], error: [] };
    this.onopen = null;
    this.onclose = null;
    this.onerror = null;
    this.onmessage = null;
    CaptureWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.readyState = CaptureWebSocket.OPEN;
      const ev = { type: "open" };
      if (typeof this.onopen === "function") this.onopen(ev);
      for (const fn of this.listeners.open) fn(ev);
    });
  }

  send(data) {
    const u8 =
      typeof data === "string"
        ? new TextEncoder().encode(data)
        : data instanceof ArrayBuffer
          ? new Uint8Array(data)
          : data instanceof Uint8Array
            ? data
            : new Uint8Array(data);
    CaptureWebSocket.frames.push(Uint8Array.from(u8));
  }

  close(code = 1000, reason = "") {
    this.readyState = CaptureWebSocket.CLOSED;
    const ev = { type: "close", code, reason, wasClean: true };
    if (typeof this.onclose === "function") this.onclose(ev);
    for (const fn of this.listeners.close) fn(ev);
  }

  addEventListener(type, fn) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(fn);
  }

  removeEventListener(type, fn) {
    if (!this.listeners[type]) return;
    this.listeners[type] = this.listeners[type].filter((x) => x !== fn);
  }

  dispatchEvent() {
    return true;
  }
}
CaptureWebSocket.CONNECTING = 0;
CaptureWebSocket.OPEN = 1;
CaptureWebSocket.CLOSING = 2;
CaptureWebSocket.CLOSED = 3;

function tryYSync(bytes, label) {
  const out = { label, bytes: bytes.byteLength, hex32: hexHead(bytes) };
  try {
    const decoder = decoding.createDecoder(bytes);
    const encoder = encoding.createEncoder();
    const doc = new Y.Doc({ gc: false });
    const msgType = syncProtocol.readSyncMessage(decoder, encoder, doc, "probe");
    out.ok = true;
    out.ySyncMessageType = msgType;
    out.replyBytes = encoding.toUint8Array(encoder).byteLength;
  } catch (e) {
    out.ok = false;
    out.error = String(e && e.message ? e.message : e);
  }
  return out;
}

function parseHocuspocusFrame(bytes) {
  const decoder = decoding.createDecoder(bytes);
  const documentName = decoding.readVarString(decoder);
  const type = decoding.readVarUint(decoder);
  const rest = bytes.subarray(decoder.pos);
  const typeName =
    Object.entries(MessageType).find(([, v]) => v === type)?.[0] ?? String(type);
  const parsed = {
    documentName,
    hocuspocusType: type,
    hocuspocusTypeName: typeName,
    restBytes: rest.byteLength,
    restHex32: hexHead(rest),
  };
  if (type === MessageType.Auth) {
    try {
      parsed.authMessageType = decoding.readVarUint(decoder);
      parsed.token = decoding.readVarString(decoder);
      parsed.providerVersion = decoding.readVarString(decoder);
    } catch (e) {
      parsed.tokenError = String(e);
    }
  }
  if (type === MessageType.Sync || type === MessageType.SyncReply) {
    parsed.innerSync = tryYSync(rest, "payload-after-hocuspocus-type");
  }
  return parsed;
}

function yProtocolsFirstVarUint(bytes) {
  try {
    const decoder = decoding.createDecoder(bytes);
    return decoding.readVarUint(decoder);
  } catch (e) {
    return { error: String(e) };
  }
}

async function captureProviderFrames() {
  CaptureWebSocket.frames = [];
  const ydoc = new Y.Doc({ gc: false });
  const provider = new HocuspocusProvider({
    url: "ws://127.0.0.1:9/collab",
    name: DOC_NAME,
    document: ydoc,
    token: String(ydoc.clientID),
    WebSocketPolyfill: CaptureWebSocket,
  });
  await new Promise((r) => setTimeout(r, 400));
  const frames = CaptureWebSocket.frames.map((f) => Uint8Array.from(f));
  provider.destroy();
  ydoc.destroy();
  return frames;
}

async function main() {
  const frames = await captureProviderFrames();
  const analyses = frames.map((bytes, i) => {
    const rawSync = tryYSync(bytes, "raw-frame-as-y-protocols-sync");
    let hocus = { error: "parse failed" };
    try {
      hocus = parseHocuspocusFrame(bytes);
    } catch (e) {
      hocus = { error: String(e && e.message ? e.message : e) };
    }
    return {
      index: i,
      byteLength: bytes.byteLength,
      firstVarUintIfYProtocols: yProtocolsFirstVarUint(bytes),
      yProtocolsSyncOnRawFrame: rawSync,
      hocuspocusDecode: hocus,
    };
  });

  const mismatch = analyses.some(
    (a) => a.yProtocolsSyncOnRawFrame && a.yProtocolsSyncOnRawFrame.ok === false,
  );
  const anyFrame = frames.length > 0;

  const missingAdapter = [
    {
      id: "document-name-prefix",
      need: "Read lib0 varString document name before any y-protocols message type.",
      why: "Hocuspocus multiplexes documents on one socket. y-protocols/sync treats the first varUint as SyncStep1/2/Update (0/1/2).",
    },
    {
      id: "hocuspocus-message-type",
      need: "Dispatch provider MessageType (Sync=0, Awareness=1, Auth=2, QueryAwareness=3, Stateless=5, CLOSE=7, SyncStatus=8, Ping=9, Pong=10).",
      why: "After the document name, Hocuspocus writes its own type. Feeding that byte to readSyncMessage confuses Sync (0) with SyncStep1.",
    },
    {
      id: "auth-token",
      need: "Handle MessageType.Auth varString token (FVOCI uses this as clientId claim). Session cookie is on the WebSocket upgrade, not in Y sync.",
      why: "y-protocols auth is a different PermissionDenied encoder, not Hocuspocus Auth.",
    },
    {
      id: "sync-status-ping-pong",
      need: "Handle MessageType.SyncStatus (8), Ping (9), Pong (10). y-protocols 1.0.7 has none of these.",
      why: "Hocuspocus provider 4.6.0 uses these for connection liveness and sync status, not Y.Doc updates.",
    },
    {
      id: "stateless-control",
      need: "Handle MessageType.Stateless (5) JSON control (FVOCI persist/persisted/persist-failed).",
      why: "Not a Y.Doc update. Yrs apply_update cannot interpret these frames.",
    },
    {
      id: "awareness-wrap",
      need: "Awareness payloads still sit behind the document-name + MessageType.Awareness wrapper.",
      why: "Bare y-protocols/awareness.decodeAwarenessUpdate expects the inner payload only.",
    },
  ];

  const report = {
    pins: {
      yjs: "13.6.32",
      "y-protocols": "1.0.7",
      "@hocuspocus/provider": "4.6.0",
      "@hocuspocus/common": "4.6.0",
      gc: false,
    },
    MessageType,
    capturedFrames: frames.length,
    mismatchIfRawYSync: mismatch,
    providerCaptured: anyFrame,
    note: anyFrame
      ? "Raw Hocuspocus provider bytes are not y-protocols sync frames. Yrs updateV1 apply is not a provider adapter."
      : "Provider sent no frames (capture failed). Decoder comparison below still documents the adapter gap using the public frame layout.",
    analyses,
    missingAdapter,
    untested: [
      "Two real FVOCI UI clients",
      "fvoci_session cookie / onAuthenticate",
      "Collab server restart + reconnect",
      "Permission revoke / clientId camping",
      "Hocuspocus extension-redis patched path",
    ],
  };

  // If the provider did not emit frames (API mismatch), still encode a
  // canonical handshake with the same library primitives and show the decode gap.
  if (!anyFrame) {
    const ydoc = new Y.Doc({ gc: false });
    const enc = encoding.createEncoder();
    encoding.writeVarString(enc, DOC_NAME);
    encoding.writeVarUint(enc, MessageType.Auth);
    writeAuthentication(enc, String(ydoc.clientID));
    encoding.writeVarString(enc, "4.6.0");
    const auth = encoding.toUint8Array(enc);
    const enc2 = encoding.createEncoder();
    encoding.writeVarString(enc2, DOC_NAME);
    encoding.writeVarUint(enc2, MessageType.Sync);
    syncProtocol.writeSyncStep1(enc2, ydoc);
    const sync = encoding.toUint8Array(enc2);
    report.synthesizedFromSameCodecs = [
      {
        kind: "Auth",
        yProtocolsSyncOnRawFrame: tryYSync(auth, "raw-auth"),
        hocuspocusDecode: parseHocuspocusFrame(auth),
      },
      {
        kind: "SyncStep1",
        yProtocolsSyncOnRawFrame: tryYSync(sync, "raw-sync"),
        hocuspocusDecode: parseHocuspocusFrame(sync),
        innerAfterSkipNameAndType: (() => {
          const d = decoding.createDecoder(sync);
          decoding.readVarString(d);
          decoding.readVarUint(d);
          const rest = sync.subarray(d.pos);
          return tryYSync(rest, "inner-sync");
        })(),
      },
    ];
    ydoc.destroy();
  }

  console.log(JSON.stringify(report, null, 2));
  // Probe succeeds if we demonstrated the mismatch (or captured frames).
  if (!anyFrame && !report.synthesizedFromSameCodecs) process.exit(1);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
