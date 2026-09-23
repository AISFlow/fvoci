#!/usr/bin/env node
/**
 * Golden Hocuspocus 4.6.0 wire fixtures for the Rust collab codec.
 *
 * Client frames are produced by the installed @hocuspocus/provider@4.6.0
 * outgoing constructors (AuthenticationMessage, SyncStepOneMessage,
 * SyncStepTwoMessage, UpdateMessage, AwarenessMessage, StatelessMessage,
 * QueryAwarenessMessage, CloseMessage) and by sendPong. Server auth uses
 * @hocuspocus/common@4.6.0 writers. SyncStatus / CLOSE-with-reason have no
 * provider outgoing class; they follow MessageReceiver 4.6.0.
 *
 * Y.Doc clientIDs are fixed. Awareness and docs are destroyed so Node exits.
 */
import {
  makeRoutingKey,
  writeAuthenticated,
  writePermissionDenied,
  writeTokenSyncRequest,
} from "@hocuspocus/common";
import {
  HocuspocusProvider,
  MessageType,
} from "@hocuspocus/provider";
import * as encoding from "lib0/encoding";
import * as syncProtocol from "y-protocols/sync";
import * as awarenessProtocol from "y-protocols/awareness";
import * as Y from "yjs";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT = join(__dirname, "..", "fixtures", "hocus-wire.json");

const WORKSPACE = "11111111-1111-4111-8111-111111111111";
const DOCUMENT = "22222222-2222-4222-8222-222222222222";
const ROUTING_KEY = `${WORKSPACE}:document:${DOCUMENT}`;
const REQUEST_ID = "33333333-3333-4333-8333-333333333333";
const CLIENT_ID = 12345;
const SESSION_ID = "session-abc";

function hex(bytes) {
  return Buffer.from(bytes).toString("hex");
}

function waitMicrotasks(times = 8) {
  let chain = Promise.resolve();
  for (let i = 0; i < times; i += 1) {
    chain = chain.then(() => new Promise((resolve) => setImmediate(resolve)));
  }
  return chain;
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

/** Same envelope OutgoingMessage uses: varString(name) + varUint(type) + payload. */
function documentEnvelope(type, buildPayload) {
  const encoder = encoding.createEncoder();
  encoding.writeVarString(encoder, ROUTING_KEY);
  encoding.writeVarUint(encoder, type);
  buildPayload(encoder);
  return encoding.toUint8Array(encoder);
}

function connectionPong() {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MessageType.Pong);
  return encoding.toUint8Array(encoder);
}

function parseHocusFrame(bytes) {
  const decoderBytes = bytes;
  let pos = 0;
  const readVarUint = () => {
    let num = 0;
    let shift = 0;
    while (pos < decoderBytes.length) {
      const r = decoderBytes[pos];
      pos += 1;
      num += (r & 127) * 2 ** shift;
      if (r < 128) return num;
      shift += 7;
    }
    throw new Error("truncated varuint");
  };
  const readVarString = () => {
    const len = readVarUint();
    const slice = decoderBytes.subarray(pos, pos + len);
    pos += len;
    return new TextDecoder().decode(slice);
  };
  const name = readVarString();
  const type = readVarUint();
  return { name, type, rest: decoderBytes.subarray(pos) };
}

async function captureProviderOutgoing() {
  CaptureWebSocket.frames = [];
  CaptureWebSocket.instances = [];

  const ydoc = new Y.Doc({ gc: false });
  ydoc.clientID = CLIENT_ID;
  const awareness = new awarenessProtocol.Awareness(ydoc);
  awareness.setLocalState({ name: "테스트✨", color: "#ff00ff" });

  const provider = new HocuspocusProvider({
    url: "ws://127.0.0.1:9/collab",
    name: ROUTING_KEY,
    document: ydoc,
    awareness,
    token: String(ydoc.clientID),
    WebSocketPolyfill: CaptureWebSocket,
    flushDelay: false,
  });

  for (let i = 0; i < 40 && CaptureWebSocket.frames.length < 3; i += 1) {
    await waitMicrotasks(2);
  }

  const handshake = CaptureWebSocket.frames.map((frame) => Uint8Array.from(frame));
  const byType = {};
  for (const frame of handshake) {
    const parsed = parseHocusFrame(frame);
    byType[parsed.type] = frame;
  }

  CaptureWebSocket.frames = [];
  provider.sendStateless(`persist:${REQUEST_ID}`);
  const persistRequest = CaptureWebSocket.frames[0] && Uint8Array.from(CaptureWebSocket.frames[0]);

  CaptureWebSocket.frames = [];
  provider.sendStateless(`persisted:${REQUEST_ID}`);
  const persistedAck = CaptureWebSocket.frames[0] && Uint8Array.from(CaptureWebSocket.frames[0]);

  CaptureWebSocket.frames = [];
  provider.sendStateless(`persist-failed:${REQUEST_ID}`);
  const persistFailed = CaptureWebSocket.frames[0] && Uint8Array.from(CaptureWebSocket.frames[0]);

  CaptureWebSocket.frames = [];
  ydoc.getText("probe").insert(0, "안녕🚀");
  await waitMicrotasks(4);
  const update = CaptureWebSocket.frames[0] && Uint8Array.from(CaptureWebSocket.frames[0]);

  CaptureWebSocket.frames = [];
  const ws = CaptureWebSocket.instances[0];
  const pingEvent = { data: Uint8Array.of(MessageType.Ping) };
  if (ws) {
    if (typeof ws.onmessage === "function") ws.onmessage(pingEvent);
    for (const fn of ws.listeners.message ?? []) fn(pingEvent);
  }
  await waitMicrotasks(4);
  const pong = CaptureWebSocket.frames[0] && Uint8Array.from(CaptureWebSocket.frames[0]);

  CaptureWebSocket.frames = [];
  provider.destroy();
  await waitMicrotasks(4);
  const closeClient = CaptureWebSocket.frames.find((frame) => {
    try {
      return parseHocusFrame(frame).type === MessageType.CLOSE;
    } catch {
      return false;
    }
  });

  awareness.destroy();
  ydoc.destroy();

  return {
    auth: byType[MessageType.Auth],
    syncStep1: byType[MessageType.Sync],
    awareness: byType[MessageType.Awareness],
    persistRequest,
    persistedAck,
    persistFailed,
    update,
    pong,
    closeClient: closeClient && Uint8Array.from(closeClient),
  };
}

function requireFrame(captured, id) {
  if (!captured) {
    throw new Error(`provider did not emit ${id}`);
  }
  return captured;
}

async function main() {
  const captured = await captureProviderOutgoing();

  const ydoc = new Y.Doc({ gc: false });
  ydoc.clientID = CLIENT_ID;
  const awareness = new awarenessProtocol.Awareness(ydoc);
  awareness.setLocalState({ name: "테스트✨", color: "#ff00ff" });

  const authToken = requireFrame(captured.auth, "AuthenticationMessage");
  const syncStep1 = requireFrame(captured.syncStep1, "SyncStepOneMessage");
  const awarenessUpdate = requireFrame(captured.awareness, "AwarenessMessage");
  const persistRequest = requireFrame(captured.persistRequest, "StatelessMessage persist");
  const persistedAck = requireFrame(captured.persistedAck, "StatelessMessage persisted");
  const persistFailed = requireFrame(captured.persistFailed, "StatelessMessage persist-failed");
  const update = requireFrame(captured.update, "UpdateMessage");
  const capturedPong = requireFrame(captured.pong, "sendPong");
  const closeClient = requireFrame(captured.closeClient, "CloseMessage");

  const expectedPong = connectionPong();
  if (hex(capturedPong) !== hex(expectedPong)) {
    throw new Error(
      `captured sendPong ${hex(capturedPong)} != writeVarUint(Pong) ${hex(expectedPong)}`,
    );
  }

  const syncStep2 = documentEnvelope(MessageType.Sync, (enc) => {
    syncProtocol.writeSyncStep2(enc, ydoc);
  });

  const queryAwareness = documentEnvelope(MessageType.QueryAwareness, () => {});

  const authTokenRequest = documentEnvelope(MessageType.Auth, (enc) => {
    writeTokenSyncRequest(enc);
  });
  const authReadonly = documentEnvelope(MessageType.Auth, (enc) => {
    writeAuthenticated(enc, "readonly");
  });
  const authReadwrite = documentEnvelope(MessageType.Auth, (enc) => {
    writeAuthenticated(enc, "read-write");
  });
  const authDenied = documentEnvelope(MessageType.Auth, (enc) => {
    writePermissionDenied(enc, "unauthorized");
  });

  const syncStatusApplied = documentEnvelope(MessageType.SyncStatus, (enc) => {
    encoding.writeVarUint(enc, 1);
  });
  const syncStatusRejected = documentEnvelope(MessageType.SyncStatus, (enc) => {
    encoding.writeVarUint(enc, 0);
  });

  const closeWithReason = documentEnvelope(MessageType.CLOSE, (enc) => {
    encoding.writeVarString(enc, "provider_initiated");
  });

  const sessionRoutingKey = makeRoutingKey(ROUTING_KEY, SESSION_ID);
  const sessionAwareness = (() => {
    const encoder = encoding.createEncoder();
    encoding.writeVarString(encoder, sessionRoutingKey);
    encoding.writeVarUint(encoder, MessageType.Awareness);
    encoding.writeVarUint8Array(
      encoder,
      awarenessProtocol.encodeAwarenessUpdate(awareness, [ydoc.clientID]),
    );
    return encoding.toUint8Array(encoder);
  })();

  const unknownType = documentEnvelope(99, () => {});
  const documentPing = documentEnvelope(MessageType.Ping, () => {});
  const documentPong = documentEnvelope(MessageType.Pong, () => {});

  const fixtures = {
    pins: {
      "@hocuspocus/provider": "4.6.0",
      "@hocuspocus/common": "4.6.0",
      yjs: "13.6.32",
      "y-protocols": "1.0.7",
      lib0: "peer",
    },
    routingKey: ROUTING_KEY,
    clientID: CLIENT_ID,
    cases: [
      {
        id: "connection_ping",
        origin: "control",
        constructor: "MessageType.Ping single byte (HocuspocusProviderWebsocket.onMessage)",
        hex: hex([MessageType.Ping]),
      },
      {
        id: "connection_pong",
        origin: "provider",
        constructor: "HocuspocusProviderWebsocket.sendPong writeVarUint(Pong)",
        hex: hex(capturedPong),
      },
      {
        id: "auth_token_client",
        origin: "provider",
        constructor: "AuthenticationMessage",
        hex: hex(authToken),
      },
      {
        id: "auth_token_request_server",
        origin: "common",
        constructor: "writeTokenSyncRequest",
        hex: hex(authTokenRequest),
      },
      {
        id: "auth_authenticated_readonly",
        origin: "common",
        constructor: "writeAuthenticated(readonly)",
        hex: hex(authReadonly),
      },
      {
        id: "auth_authenticated_readwrite",
        origin: "common",
        constructor: "writeAuthenticated(read-write)",
        hex: hex(authReadwrite),
      },
      {
        id: "auth_permission_denied",
        origin: "common",
        constructor: "writePermissionDenied",
        hex: hex(authDenied),
      },
      {
        id: "sync_step1",
        origin: "provider",
        constructor: "SyncStepOneMessage",
        hex: hex(syncStep1),
      },
      {
        id: "sync_step2",
        origin: "provider",
        constructor: "SyncStepTwoMessage.get (MessageType.Sync, not a separate opcode)",
        hex: hex(syncStep2),
      },
      {
        id: "sync_update_korean_emoji",
        origin: "provider",
        constructor: "UpdateMessage",
        hex: hex(update),
      },
      {
        id: "awareness_update",
        origin: "provider",
        constructor: "AwarenessMessage",
        hex: hex(awarenessUpdate),
      },
      {
        id: "query_awareness",
        origin: "provider",
        constructor: "QueryAwarenessMessage.get",
        hex: hex(queryAwareness),
      },
      {
        id: "stateless_persist",
        origin: "provider",
        constructor: "StatelessMessage sendStateless",
        hex: hex(persistRequest),
      },
      {
        id: "stateless_persisted",
        origin: "provider",
        constructor: "StatelessMessage sendStateless",
        hex: hex(persistedAck),
      },
      {
        id: "stateless_persist_failed",
        origin: "provider",
        constructor: "StatelessMessage sendStateless",
        hex: hex(persistFailed),
      },
      {
        id: "sync_status_applied",
        origin: "server",
        constructor: "MessageReceiver SyncStatus readVarInt===1",
        hex: hex(syncStatusApplied),
      },
      {
        id: "sync_status_rejected",
        origin: "server",
        constructor: "MessageReceiver SyncStatus readVarInt===1",
        hex: hex(syncStatusRejected),
      },
      {
        id: "close_server_reason",
        origin: "server",
        constructor: "MessageReceiver CLOSE readVarString(reason)",
        hex: hex(closeWithReason),
      },
      {
        id: "close_client",
        origin: "provider",
        constructor: "CloseMessage",
        hex: hex(closeClient),
      },
      {
        id: "awareness_session_routing_key",
        origin: "provider",
        constructor: "makeRoutingKey + AwarenessMessage.get",
        hex: hex(sessionAwareness),
        routingKey: sessionRoutingKey,
      },
    ],
    malformed: [
      { id: "empty", hex: "" },
      { id: "truncated_varstring", hex: "fd" },
      { id: "unknown_message_type", hex: hex(unknownType) },
      { id: "oversized_length_prefix", hex: "ff" + "80".repeat(12) },
      { id: "document_ping", hex: hex(documentPing) },
      { id: "document_pong", hex: hex(documentPong) },
      { id: "varuint_overflow", hex: "ff".repeat(9) + "03" },
    ],
  };

  writeFileSync(OUT, JSON.stringify(fixtures, null, 2) + "\n");
  console.log(`wrote ${OUT} (${fixtures.cases.length} golden + ${fixtures.malformed.length} malformed)`);

  awareness.destroy();
  ydoc.destroy();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
