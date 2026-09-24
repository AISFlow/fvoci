import assert from "node:assert/strict";
import test from "node:test";
import { peersEqual, peersFromStates } from "./collab-model.ts";

const me = { id: "u-me", name: "나", color: "#1d4ed8" };
const ada = { id: "u-ada", name: "Ada", color: "#b91c1c" };

test("같은 user.id 의 두 clientId 는 피어 2개, self 는 내 id 만, 내 clientId 제외, cursor 없으면 blockId 없음", () => {
  const states = new Map<number, unknown>([
    [1, { user: me, block: { id: "b1" }, cursor: {} }],
    [2, { user: me, block: { id: "b2" }, cursor: {} }],
    [3, { user: ada, block: { id: "b3" } }],
    [4, { user: ada, block: null, cursor: {} }],
    [5, { cursor: {} }],
    [6, { user: { name: "이름만" } }],
  ]);
  assert.deepEqual(peersFromStates(states, 1, me.id), [
    {
      clientId: 2,
      id: "u-me",
      name: "나",
      color: "#1d4ed8",
      blockId: "b2",
      titleEditing: false,
      self: true,
    },
    {
      clientId: 3,
      id: "u-ada",
      name: "Ada",
      color: "#b91c1c",
      blockId: null,
      titleEditing: false,
      self: false,
    },
    {
      clientId: 4,
      id: "u-ada",
      name: "Ada",
      color: "#b91c1c",
      blockId: null,
      titleEditing: false,
      self: false,
    },
  ]);
});

test("color: red;background:url(x) 피어는 peersFromStates 결과 0", () => {
  const states = new Map<number, unknown>([
    [
      2,
      {
        user: {
          id: "u-evil",
          name: "x",
          color: "red;background:url(x)",
        },
      },
    ],
  ]);
  assert.deepEqual(peersFromStates(states, 1, "u-me"), []);
});

const sameStates = () =>
  new Map<number, unknown>([
    [2, { user: me, block: { id: "b1" }, cursor: {} }],
    [3, { user: ada, block: { id: "b3" }, cursor: {} }],
  ]);

test("같은 상태 맵에서 나온 두 결과는 참조가 달라도 peersEqual", () => {
  const first = peersFromStates(sameStates(), 1, me.id);
  const second = peersFromStates(sameStates(), 1, me.id);
  assert.notEqual(first, second);
  assert.equal(peersEqual(first, second), true);
});

test("blockId 만 바뀌어도 peersEqual 은 false", () => {
  const before = peersFromStates(sameStates(), 1, me.id);
  const moved = sameStates();
  moved.set(3, { user: ada, block: { id: "b9" }, cursor: {} });
  assert.equal(peersEqual(before, peersFromStates(moved, 1, me.id)), false);
});

test("피어가 늘면 peersEqual 은 false", () => {
  const before = peersFromStates(sameStates(), 1, me.id);
  const joined = sameStates();
  joined.set(4, { user: ada, block: null, cursor: {} });
  assert.equal(peersEqual(before, peersFromStates(joined, 1, me.id)), false);
});

test("내 clientId 는 제외되므로 로컬 커서만 움직이면 목록이 같다", () => {
  const before = peersFromStates(sameStates(), 1, me.id);
  const localMoved = sameStates();
  localMoved.set(1, { user: me, block: { id: "b7" }, cursor: {} });
  assert.equal(peersEqual(before, peersFromStates(localMoved, 1, me.id)), true);
});

test("title:true 만 제목 편집으로 읽고 문자열은 버린다", () => {
  const states = new Map<number, unknown>([
    [2, { user: ada, title: true }],
    [3, { user: me, title: "stolen title" }],
  ]);
  assert.deepEqual(peersFromStates(states, 1, me.id), [
    {
      clientId: 2,
      id: "u-ada",
      name: "Ada",
      color: "#b91c1c",
      blockId: null,
      titleEditing: true,
      self: false,
    },
    {
      clientId: 3,
      id: "u-me",
      name: "나",
      color: "#1d4ed8",
      blockId: null,
      titleEditing: false,
      self: true,
    },
  ]);
});

test("titleEditing 만 바뀌어도 peersEqual 은 false", () => {
  const before = peersFromStates(sameStates(), 1, me.id);
  const editing = sameStates();
  editing.set(3, { user: ada, block: { id: "b3" }, cursor: {}, title: true });
  assert.equal(peersEqual(before, peersFromStates(editing, 1, me.id)), false);
});

test("self 만 달라져도 peersEqual 은 false", () => {
  const mine = peersFromStates(sameStates(), 1, me.id);
  const asAda = peersFromStates(sameStates(), 1, ada.id);
  assert.deepEqual(
    mine.map((peer) => peer.self),
    [true, false],
  );
  assert.deepEqual(
    asAda.map((peer) => peer.self),
    [false, true],
  );
  assert.equal(peersEqual(mine, asAda), false);
});
