import assert from "node:assert/strict";
import test from "node:test";
import * as Y from "yjs";
import {
  applyPersistAck,
  createPersistAck,
  isDurablySaved,
} from "./collab-persist-ack.ts";

function vectorsEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i += 1) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

test("connected sync without persist ack is not saved", () => {
  const state = createPersistAck("doc-a", "conn-1");
  assert.equal(isDurablySaved(state), false);
});

test("matching persist ack for the current prefix is saved", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), true);
});

test("delayed ack after a new edit cannot show saved", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), false);
});

test("wrong requestId does not confirm, later matching id still can", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "ack", requestId: "foreign" });
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), true);
});

test("late ack after a newer requestId cannot show saved", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-old" });
  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(state, { type: "request", requestId: "req-new" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-old" });
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, { type: "ack", requestId: "req-new" });
  assert.equal(isDurablySaved(state), true);
});

test("timeout then late matching ack cannot show saved", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "timeout", requestId: "req-1" });
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), false);
});

test("persist-failed cannot show saved", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "fail", requestId: "req-1" });
  assert.equal(isDurablySaved(state), false);
});

test("timeout or fail for a foreign requestId leaves the in-flight save pending", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "timeout", requestId: "other" });
  state = applyPersistAck(state, { type: "fail", requestId: "other" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), true);
});

test("delete-only invalidates saved even when the state vector is unchanged", () => {
  const doc = new Y.Doc({ gc: false });
  const text = doc.getText("t");
  let state = createPersistAck("doc-a", "conn-1");
  const onUpdate = () => {
    state = applyPersistAck(state, { type: "edit" });
  };
  doc.on("update", onUpdate);
  text.insert(0, "지울 문장");
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
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
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, {
    type: "bind",
    documentId: "doc-a",
    connectionId: "conn-2",
  });
  assert.equal(isDurablySaved(state), false);

  state = applyPersistAck(state, { type: "edit" });
  state = applyPersistAck(state, { type: "request", requestId: "req-2" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-2" });
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, {
    type: "bind",
    documentId: "doc-b",
    connectionId: "conn-2",
  });
  assert.equal(isDurablySaved(state), false);
});

test("repeated save confirms only the latest matching prefix", () => {
  let state = applyPersistAck(createPersistAck("doc-a", "conn-1"), {
    type: "edit",
  });
  state = applyPersistAck(state, { type: "request", requestId: "req-1" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, { type: "edit" });
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, { type: "request", requestId: "req-2" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-1" });
  assert.equal(isDurablySaved(state), false);
  state = applyPersistAck(state, { type: "ack", requestId: "req-2" });
  assert.equal(isDurablySaved(state), true);

  state = applyPersistAck(state, { type: "request", requestId: "req-3" });
  state = applyPersistAck(state, { type: "ack", requestId: "req-3" });
  assert.equal(isDurablySaved(state), true);
});
