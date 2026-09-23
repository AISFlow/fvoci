import assert from "node:assert/strict";
import test from "node:test";
import { collabBadge } from "./collab-badge.ts";

test("connected 라도 미전송 변경이 남으면 「저장됨」이 아니라 저장 대기다", () => {
  assert.equal(collabBadge("connected", false).label, "doc.collab.saved");
  assert.equal(collabBadge("connected", false).tone, "live");
  assert.equal(collabBadge("connected", true).label, "doc.collab.pending");
  assert.equal(collabBadge("connected", true).tone, "wait");
});

test("연결 밖 상태는 미전송 변경과 무관하게 그 상태 배지다", () => {
  assert.equal(collabBadge("connecting", false).label, "doc.collab.connecting");
  assert.equal(collabBadge("disconnected", true).label, "doc.collab.reconnecting");
  assert.equal(collabBadge("unauthorized", false).label, "doc.collab.unauthorized");
  assert.equal(collabBadge("unauthorized", false).tone, "danger");
});
