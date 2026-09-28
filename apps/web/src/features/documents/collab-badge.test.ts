import assert from "node:assert/strict";
import test from "node:test";
import { collabBadge, collabRefusalNote } from "./collab-badge.ts";

test("connected 배지는 persist ack 없이 「저장됨」을 쓰지 않는다", () => {
  assert.equal(collabBadge("connected", false).label, "doc.collab.connected");
  assert.equal(collabBadge("connected", false).tone, "live");
  assert.equal(collabBadge("connected", false, false).label, "doc.collab.connected");
  assert.equal(collabBadge("connected", false, true).label, "doc.collab.saved");
  assert.equal(collabBadge("connected", false, true).tone, "live");
  assert.equal(collabBadge("connected", true).label, "doc.collab.pending");
  assert.equal(collabBadge("connected", true).tone, "wait");
  assert.equal(collabBadge("connected", true, true).label, "doc.collab.pending");
});

test("연결 밖 상태는 미전송 변경·persist ack 과 무관하게 그 상태 배지다", () => {
  assert.equal(collabBadge("connecting", false).label, "doc.collab.connecting");
  assert.equal(collabBadge("connecting", false, true).label, "doc.collab.connecting");
  assert.equal(collabBadge("disconnected", true).label, "doc.collab.reconnecting");
  assert.equal(collabBadge("unauthorized", false).label, "doc.collab.unauthorized");
  assert.equal(collabBadge("unauthorized", false, true).tone, "danger");
});

test("방 거절은 연결됨이 아니라 거절 배지와 「불러오지 못함」 안내다", () => {
  assert.equal(collabBadge("busy", false).label, "doc.collab.busy");
  assert.equal(collabBadge("busy", false, true).tone, "danger");
  assert.equal(collabBadge("unavailable", true).label, "doc.collab.unavailable");
  assert.equal(collabRefusalNote("busy"), "doc.collab.busyNote");
  assert.equal(collabRefusalNote("unavailable"), "doc.collab.unavailableNote");
  for (const status of ["connected", "connecting", "disconnected", "unauthorized", undefined] as const) {
    assert.equal(collabRefusalNote(status), null);
  }
});
