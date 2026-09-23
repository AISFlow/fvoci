#!/usr/bin/env node
/**
 * Golden Hocuspocus 4.6.0 wire fixtures for the Rust collab codec.
 * Uses pinned compat/js package-lock serialization only (lib0 + provider/common).
 */
import { writeAuthentication, writeAuthenticated, writePermissionDenied, writeTokenSyncRequest } from "@hocuspocus/common";
import { MessageType } from "@hocuspocus/provider";
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

function hex(bytes) {
  return Buffer.from(bytes).toString("hex");
}

function documentFrame(type, buildPayload) {
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

function main() {
  const ydoc = new Y.Doc({ gc: false });
  const awareness = new awarenessProtocol.Awareness(ydoc);

  const syncStep1 = documentFrame(MessageType.Sync, (enc) => {
    syncProtocol.writeSyncStep1(enc, ydoc);
  });

  const syncStep2 = documentFrame(MessageType.Sync, (enc) => {
    syncProtocol.writeSyncStep2(enc, ydoc);
  });

  const update = documentFrame(MessageType.Sync, (enc) => {
    const patch = new Y.Doc({ gc: false });
    patch.getText("probe").insert(0, "안녕🚀");
    syncProtocol.writeUpdate(enc, Y.encodeStateAsUpdate(patch));
    patch.destroy();
  });

  const authToken = documentFrame(MessageType.Auth, (enc) => {
    writeAuthentication(enc, String(ydoc.clientID));
    encoding.writeVarString(enc, "4.6.0");
  });

  const authReadonly = documentFrame(MessageType.Auth, (enc) => {
    writeAuthenticated(enc, "readonly");
  });

  const authReadwrite = documentFrame(MessageType.Auth, (enc) => {
    writeAuthenticated(enc, "read-write");
  });

  const authDenied = documentFrame(MessageType.Auth, (enc) => {
    writePermissionDenied(enc, "unauthorized");
  });

  const authTokenRequest = documentFrame(MessageType.Auth, (enc) => {
    writeTokenSyncRequest(enc);
  });

  const awarenessUpdate = documentFrame(MessageType.Awareness, (enc) => {
    awareness.setLocalState({ name: "테스트✨", color: "#ff00ff" });
    const blob = awarenessProtocol.encodeAwarenessUpdate(
      awareness,
      Array.from(awareness.getStates().keys()),
    );
    encoding.writeVarUint8Array(enc, blob);
  });

  const queryAwareness = documentFrame(MessageType.QueryAwareness, () => {});

  const persistRequest = documentFrame(MessageType.Stateless, (enc) => {
    encoding.writeVarString(enc, "persist:33333333-3333-4333-8333-333333333333");
  });

  const persistedAck = documentFrame(MessageType.Stateless, (enc) => {
    encoding.writeVarString(enc, "persisted:33333333-3333-4333-8333-333333333333");
  });

  const persistFailed = documentFrame(MessageType.Stateless, (enc) => {
    encoding.writeVarString(enc, "persist-failed:33333333-3333-4333-8333-333333333333");
  });

  const syncStatusApplied = documentFrame(MessageType.SyncStatus, (enc) => {
    encoding.writeVarUint(enc, 1);
  });

  const syncStatusRejected = documentFrame(MessageType.SyncStatus, (enc) => {
    encoding.writeVarUint(enc, 0);
  });

  const closeWithReason = documentFrame(MessageType.CLOSE, (enc) => {
    encoding.writeVarString(enc, "provider_initiated");
  });

  const closeClient = documentFrame(MessageType.CLOSE, () => {});

  const SYNC_REPLY = 4;
  const BROADCAST_STATELESS = 6;

  const syncReply = documentFrame(SYNC_REPLY, (enc) => {
    syncProtocol.writeSyncStep2(enc, ydoc);
  });

  const broadcastStateless = documentFrame(BROADCAST_STATELESS, (enc) => {
    encoding.writeVarString(enc, "server-internal");
  });

  const sessionRoutingKey = `${ROUTING_KEY}\0session-abc`;
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

  const fixtures = {
    pins: {
      "@hocuspocus/provider": "4.6.0",
      "@hocuspocus/common": "4.6.0",
      yjs: "13.6.32",
      "y-protocols": "1.0.7",
      lib0: "peer",
    },
    routingKey: ROUTING_KEY,
    cases: [
      { id: "connection_ping", hex: hex([MessageType.Ping]) },
      { id: "connection_pong", hex: hex(connectionPong()) },
      { id: "auth_token_client", hex: hex(authToken) },
      { id: "auth_token_request_server", hex: hex(authTokenRequest) },
      { id: "auth_authenticated_readonly", hex: hex(authReadonly) },
      { id: "auth_authenticated_readwrite", hex: hex(authReadwrite) },
      { id: "auth_permission_denied", hex: hex(authDenied) },
      { id: "sync_step1", hex: hex(syncStep1) },
      { id: "sync_step2", hex: hex(syncStep2) },
      { id: "sync_update_korean_emoji", hex: hex(update) },
      { id: "sync_reply_step2", hex: hex(syncReply) },
      { id: "awareness_update", hex: hex(awarenessUpdate) },
      { id: "query_awareness", hex: hex(queryAwareness) },
      { id: "stateless_persist", hex: hex(persistRequest) },
      { id: "stateless_persisted", hex: hex(persistedAck) },
      { id: "stateless_persist_failed", hex: hex(persistFailed) },
      { id: "sync_status_applied", hex: hex(syncStatusApplied) },
      { id: "sync_status_rejected", hex: hex(syncStatusRejected) },
      { id: "close_server_reason", hex: hex(closeWithReason) },
      { id: "close_client", hex: hex(closeClient) },
      { id: "broadcast_stateless_server", hex: hex(broadcastStateless) },
      { id: "awareness_session_routing_key", hex: hex(sessionAwareness), routingKey: sessionRoutingKey },
    ],
    malformed: [
      { id: "empty", hex: "" },
      { id: "truncated_varstring", hex: "fd" },
      { id: "unknown_message_type", hex: hex(documentFrame(99, () => {})) },
      { id: "oversized_length_prefix", hex: "ff" + "80".repeat(12) },
    ],
  };

  writeFileSync(OUT, JSON.stringify(fixtures, null, 2) + "\n");
  console.log(`wrote ${OUT} (${fixtures.cases.length} golden + ${fixtures.malformed.length} malformed)`);
  ydoc.destroy();
}

main();
