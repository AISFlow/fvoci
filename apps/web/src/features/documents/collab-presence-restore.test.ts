import assert from "node:assert/strict";
import test from "node:test";
import { Awareness, removeAwarenessStates } from "y-protocols/awareness";
import * as Y from "yjs";
import { reassertPresence } from "./collab-model.ts";

const me = { id: "u-me", name: "나", color: "#1d4ed8" };

test("provider 의 pagehide 처리 뒤 setLocalStateField 는 프레즌스를 못 되살린다", () => {
  const awareness = new Awareness(new Y.Doc());
  awareness.setLocalState({ user: me, block: { id: "b1" } });
  removeAwarenessStates(awareness, [awareness.clientID], "page hide");
  awareness.setLocalStateField("user", me);
  assert.equal(awareness.getLocalState(), null);
  awareness.destroy();
});

test("reassertPresence 는 user 와 마지막 block 을 함께 되살린다", () => {
  const awareness = new Awareness(new Y.Doc());
  awareness.setLocalState({ user: me, block: { id: "b1" } });
  removeAwarenessStates(awareness, [awareness.clientID], "page hide");
  reassertPresence(awareness, me, "b1");
  assert.deepEqual(awareness.getStates().get(awareness.clientID), {
    user: me,
    block: { id: "b1" },
  });
  awareness.destroy();
});

test("reassertPresence 는 캐럿 블록이 없으면 user 만 넣는다", () => {
  const awareness = new Awareness(new Y.Doc());
  awareness.setLocalState({ user: me, block: { id: "b1" } });
  removeAwarenessStates(awareness, [awareness.clientID], "page hide");
  reassertPresence(awareness, me, null);
  assert.deepEqual(awareness.getLocalState(), { user: me });
  awareness.destroy();
});

test("reassertPresence 는 제목 편집 플래그를 되살린다", () => {
  const awareness = new Awareness(new Y.Doc());
  awareness.setLocalState({ user: me, block: { id: "b1" } });
  removeAwarenessStates(awareness, [awareness.clientID], "page hide");
  reassertPresence(awareness, me, null, true);
  assert.deepEqual(awareness.getLocalState(), { user: me, title: true });
  awareness.destroy();
});

test("reassertPresence 는 살아 있는 로컬 상태를 덮지 않는다", () => {
  const awareness = new Awareness(new Y.Doc());
  awareness.setLocalState({ user: me, block: { id: "b1" } });
  awareness.setLocalStateField("cursor", { anchor: 1 });
  reassertPresence(awareness, me, null);
  assert.deepEqual(awareness.getLocalState(), {
    user: me,
    block: { id: "b1" },
    cursor: { anchor: 1 },
  });
  awareness.destroy();
});
