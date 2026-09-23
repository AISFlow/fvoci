import assert from "node:assert/strict";
import test from "node:test";
import {
  COLLAB_PERSIST_DONE,
  COLLAB_PERSIST_REQUEST,
} from "@fvoci/editor/collab";
import type { HocuspocusProvider, onStatelessParameters } from "@hocuspocus/provider";
import * as Y from "yjs";
import { PERSIST_DISCONNECTED_MESSAGE, persistNow } from "./collab-model.ts";
import {
  advanceConnectionGeneration,
  applyPersistAck,
  createConnectionGeneration,
  createPersistAck,
  isDurablySaved,
  reducePersistBind,
  scopedPersistObserver,
  syncPersistBind,
  type PersistAckEvent,
  type PersistAckScope,
  type PersistBindState,
} from "./collab-persist-ack.ts";

const DOC = "doc-a";
const CONN = "conn-1";
const ROOM: PersistAckScope = { documentId: DOC, connectionId: CONN };

function vectorsEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i += 1) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function request(
  requestId: string,
  scope: PersistAckScope = ROOM,
): PersistAckEvent {
  return { type: "request", requestId, ...scope };
}

function ack(requestId: string, scope: PersistAckScope = ROOM): PersistAckEvent {
  return { type: "ack", requestId, ...scope };
}

function fail(requestId: string, scope: PersistAckScope = ROOM): PersistAckEvent {
  return { type: "fail", requestId, ...scope };
}

function timeout(
  requestId: string,
  scope: PersistAckScope = ROOM,
): PersistAckEvent {
  return { type: "timeout", requestId, ...scope };
}

function fakeProvider(): {
  provider: HocuspocusProvider;
  calls: string[];
  emit(payload: string): void;
} {
  const listeners = new Set<(params: onStatelessParameters) => void>();
  const calls: string[] = [];
  const provider = {
    flushPendingUpdates() {
      calls.push("flush");
    },
    sendStateless(payload: string) {
      calls.push(`stateless:${payload}`);
    },
    on(event: string, fn: (params: onStatelessParameters) => void) {
      if (event === "stateless") listeners.add(fn);
      return provider;
    },
    off(event: string, fn: (params: onStatelessParameters) => void) {
      if (event === "stateless") listeners.delete(fn);
      return provider;
    },
  } as unknown as HocuspocusProvider;
  return {
    provider,
    calls,
    emit(payload: string) {
      for (const listener of listeners) listener({ payload });
    },
  };
}

function nextIds(...ids: string[]): () => string {
  let i = 0;
  return () => {
    const id = ids[i];
    i += 1;
    assert.ok(id, "connection generation asked for more ids than the test provided");
    return id;
  };
}

test("connected sync without persist ack is not saved", () => {
  const state = createPersistAck(DOC, CONN);
  assert.equal(isDurablySaved(state), false);
});

test("matching persist ack for the current prefix is saved", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);
});

test("delayed ack after a new edit cannot show saved", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), false);
});

test("wrong requestId does not confirm, later matching id still can", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, ack("foreign"));
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);
});

test("late ack after a newer requestId cannot show saved", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-old"));
  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(state, request("req-new"));
  state = applyPersistAck(state, ack("req-old"));
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, ack("req-new"));
  assert.equal(isDurablySaved(state), true);
});

test("timeout then late matching ack cannot show saved", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, timeout("req-1"));
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), false);
});

test("persist-failed cannot show saved", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, fail("req-1"));
  assert.equal(isDurablySaved(state), false);
});

test("timeout or fail for a foreign requestId leaves the in-flight save pending", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, timeout("other"));
  state = applyPersistAck(state, fail("other"));
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);
});

test("delete-only invalidates saved even when the state vector is unchanged", () => {
  const doc = new Y.Doc({ gc: false });
  const text = doc.getText("t");
  let state = createPersistAck(DOC, CONN);
  const onUpdate = () => {
    state = applyPersistAck(state, { type: "edit" });
  };
  doc.on("update", onUpdate);
  text.insert(0, "지울 문장");
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);
  const savedVector = Y.encodeStateVector(doc);

  text.delete(0, text.length);
  const afterVector = Y.encodeStateVector(doc);
  assert.equal(vectorsEqual(savedVector, afterVector), true);
  assert.equal(text.toString(), "");
  assert.equal(isDurablySaved(state), false);
  doc.off("update", onUpdate);
  doc.destroy();
});

test("reconnect or a new room drops a previous persist confirmation", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, {
    type: "bind",
    documentId: DOC,
    connectionId: "conn-2",
  });
  assert.equal(isDurablySaved(state), false);

  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(state, request("req-2", { documentId: DOC, connectionId: "conn-2" }));
  state = applyPersistAck(state, ack("req-2", { documentId: DOC, connectionId: "conn-2" }));
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, {
    type: "bind",
    documentId: "doc-b",
    connectionId: "conn-2",
  });
  assert.equal(isDurablySaved(state), false);
});

test("repeated save confirms only the latest matching prefix", () => {
  let state = applyPersistAck(createPersistAck(DOC, CONN), { type: "edit" });
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, { type: "edit" });
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, request("req-2"));
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, ack("req-2"));
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, request("req-3"));
  state = applyPersistAck(state, ack("req-3"));
  assert.equal(isDurablySaved(state), true);
});

test("same provider object reconnect rotates connection generation", () => {
  const provider = { id: "same-instance" };
  const ids = nextIds("g1", "g2", "g3");
  let gen = createConnectionGeneration(provider, "connecting", ids);
  assert.equal(gen.connectionId, "g1");

  gen = advanceConnectionGeneration(gen, { provider, status: "connected" }, ids);
  assert.equal(gen.connectionId, "g1");

  gen = advanceConnectionGeneration(gen, { provider, status: "disconnected" }, ids);
  assert.equal(gen.connectionId, "g2");

  gen = advanceConnectionGeneration(gen, { provider, status: "connecting" }, ids);
  assert.equal(gen.connectionId, "g2");
  gen = advanceConnectionGeneration(gen, { provider, status: "connected" }, ids);
  assert.equal(gen.connectionId, "g2");

  gen = advanceConnectionGeneration(gen, { provider, status: "connecting" }, ids);
  assert.equal(gen.connectionId, "g3");
});

test("provider identity change rotates even without a disconnect", () => {
  const first = { id: "first" };
  const second = { id: "second" };
  const ids = nextIds("g1", "g2");
  let gen = createConnectionGeneration(first, "connected", ids);
  gen = advanceConnectionGeneration(
    gen,
    { provider: second, status: "connecting" },
    ids,
  );
  assert.equal(gen.connectionId, "g2");
  assert.equal(gen.provider, second);
});

test("same provider reconnect and delayed old persistNow ack cannot confirm saved", async () => {
  const provider = { id: "same-instance" };
  const ids = nextIds("g1", "g2");
  const doc = new Y.Doc({ gc: false });
  const text = doc.getText("t");
  text.insert(0, "미전송 한글");
  const clientId = doc.clientID;
  const pending = Y.encodeStateAsUpdate(doc);

  let bind: PersistBindState = {
    ...createConnectionGeneration(provider, "connecting", ids),
    ack: createPersistAck(DOC, "g1"),
  };
  bind = syncPersistBind(
    bind,
    { provider, status: "connected", documentId: DOC },
    ids,
  );
  assert.equal(bind.ack.connectionId, "g1");
  bind = reducePersistBind(bind, { type: "edit" });

  const fake = fakeProvider();
  const oldObserver = scopedPersistObserver(
    bind.ack.documentId,
    bind.ack.connectionId,
    (event) => {
      bind = reducePersistBind(bind, event);
    },
  );
  const abort = new AbortController();
  const inflight = persistNow(fake.provider, oldObserver, {
    signal: abort.signal,
  });
  const requestLine = fake.calls.find((call) =>
    call.startsWith(`stateless:${COLLAB_PERSIST_REQUEST}:`),
  );
  const requestId = requestLine?.slice(
    `stateless:${COLLAB_PERSIST_REQUEST}:`.length,
  );
  assert.ok(requestId);
  assert.equal(bind.ack.inflight?.requestId, requestId);

  bind = syncPersistBind(
    bind,
    { provider, status: "disconnected", documentId: DOC },
    ids,
  );
  abort.abort();
  await assert.rejects(inflight, { message: PERSIST_DISCONNECTED_MESSAGE });
  assert.equal(bind.ack.connectionId, "g2");
  assert.equal(isDurablySaved(bind.ack), false);

  bind = syncPersistBind(
    bind,
    { provider, status: "connecting", documentId: DOC },
    ids,
  );
  bind = syncPersistBind(
    bind,
    { provider, status: "connected", documentId: DOC },
    ids,
  );
  assert.equal(bind.ack.connectionId, "g2");

  fake.emit(`${COLLAB_PERSIST_DONE}:${requestId}`);
  assert.equal(isDurablySaved(bind.ack), false);

  assert.equal(doc.clientID, clientId);
  assert.equal(text.toString(), "미전송 한글");
  assert.equal(vectorsEqual(pending, Y.encodeStateAsUpdate(doc)), true);

  const newObserver = scopedPersistObserver(
    bind.ack.documentId,
    bind.ack.connectionId,
    (event) => {
      bind = reducePersistBind(bind, event);
    },
  );
  bind = reducePersistBind(bind, { type: "edit" });
  newObserver.onRequest("req-new");
  oldObserver.onAck(requestId);
  assert.equal(isDurablySaved(bind.ack), false);
  newObserver.onAck("req-new");
  assert.equal(isDurablySaved(bind.ack), true);
  oldObserver.onAck(requestId);
  oldObserver.onTimeout(requestId);
  assert.equal(isDurablySaved(bind.ack), true);

  doc.destroy();
});

test("scoped request from a previous room cannot start inflight on the current bind", () => {
  let state = createPersistAck(DOC, CONN);
  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(
    state,
    request("req-other", { documentId: "doc-b", connectionId: CONN }),
  );
  assert.equal(state.inflight, null);
  state = applyPersistAck(
    state,
    request("req-stale", { documentId: DOC, connectionId: "conn-old" }),
  );
  assert.equal(state.inflight, null);
  state = applyPersistAck(state, request("req-1"));
  state = applyPersistAck(
    state,
    ack("req-1", { documentId: DOC, connectionId: "conn-old" }),
  );
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, ack("req-1"));
  assert.equal(isDurablySaved(state), true);
});
