import assert from "node:assert/strict";
import test from "node:test";
import * as Y from "yjs";

await test("clientID reclaim keeps the same Y.Doc and unsent structs", () => {
  const doc = new Y.Doc({ gc: false });
  const before = doc.clientID;
  doc.getText("t").insert(0, "unsent");
  doc.clientID = new Y.Doc().clientID;
  assert.notEqual(doc.clientID, before);

  const peer = new Y.Doc();
  Y.applyUpdate(peer, Y.encodeStateAsUpdate(doc));
  assert.equal(peer.getText("t").toJSON(), "unsent");
  assert.equal(peer.store.clients.has(before), true);
});
