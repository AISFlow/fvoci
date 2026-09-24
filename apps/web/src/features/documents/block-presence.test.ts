import assert from "node:assert/strict";
import test from "node:test";
import {
  type BlockPresenceEditor,
  type BlockPresenceState,
  bindBlockPresence,
  blockIdAt,
} from "./block-presence.ts";
import { peersFromStates } from "./collab-model.ts";

const BLOCK_A = "0198c0de-0000-7000-8000-000000000301";
const BLOCK_B = "0198c0de-0000-7000-8000-000000000302";

function stateAt(...ids: (string | undefined)[]): BlockPresenceState {
  return {
    selection: {
      $from: {
        depth: ids.length,
        node: (depth: number) => {
          const id = ids[depth - 1];
          return { attrs: id === undefined ? {} : { id } };
        },
      },
    },
  };
}

class FakeEditor implements BlockPresenceEditor {
  state: BlockPresenceState = stateAt(BLOCK_A);
  private readonly listeners = new Set<() => void>();
  on(_event: "selectionUpdate", cb: () => void): void {
    this.listeners.add(cb);
  }
  off(_event: "selectionUpdate", cb: () => void): void {
    this.listeners.delete(cb);
  }
  moveTo(state: BlockPresenceState): void {
    this.state = state;
    for (const cb of this.listeners) cb();
  }
}

function recorder(): {
  awareness: { setLocalStateField(field: string, value: unknown): void };
  writes: [string, unknown][];
} {
  const writes: [string, unknown][] = [];
  return {
    writes,
    awareness: {
      setLocalStateField: (field, value) => {
        writes.push([field, value]);
      },
    },
  };
}

test("blockIdAt — 안쪽 블록 id 를 고르고, 없으면 null", () => {
  assert.equal(blockIdAt(stateAt(BLOCK_A)), BLOCK_A);
  assert.equal(blockIdAt(stateAt(BLOCK_A, BLOCK_B)), BLOCK_B);
  assert.equal(blockIdAt(stateAt(BLOCK_A, undefined)), BLOCK_A);
  assert.equal(blockIdAt(stateAt(undefined)), null);
  assert.equal(blockIdAt(stateAt()), null);
  assert.equal(blockIdAt(stateAt("")), null);
});

test("bindBlockPresence — 블록이 바뀔 때만 awareness 에 쓴다", () => {
  const editor = new FakeEditor();
  const { awareness, writes } = recorder();

  bindBlockPresence(editor, awareness);
  assert.deepEqual(writes, [["block", { id: BLOCK_A }]]);

  editor.moveTo(stateAt(BLOCK_A));
  assert.equal(writes.length, 1);

  editor.moveTo(stateAt(BLOCK_B));
  assert.deepEqual(writes, [
    ["block", { id: BLOCK_A }],
    ["block", { id: BLOCK_B }],
  ]);

  editor.moveTo(stateAt(undefined));
  assert.deepEqual(writes.at(-1), ["block", null]);
});

test("bindBlockPresence — 해제하면 block 을 지우고 더는 쓰지 않는다", () => {
  const editor = new FakeEditor();
  const { awareness, writes } = recorder();

  const unbind = bindBlockPresence(editor, awareness);
  unbind();
  assert.deepEqual(writes.at(-1), ["block", null]);

  const after = writes.length;
  editor.moveTo(stateAt(BLOCK_B));
  assert.equal(writes.length, after);
});

test("writer 가 쓴 상태를 peersFromStates 가 blockId 로 읽는다", () => {
  const editor = new FakeEditor();
  const local: Record<string, unknown> = {
    user: { id: "u-ada", name: "Ada", color: "#b91c1c" },
    cursor: {},
  };
  bindBlockPresence(editor, {
    setLocalStateField: (field, value) => {
      local[field] = value;
    },
  });

  assert.deepEqual(peersFromStates(new Map([[7, local]]), 1, "u-me"), [
    {
      clientId: 7,
      id: "u-ada",
      name: "Ada",
      color: "#b91c1c",
      blockId: BLOCK_A,
      titleEditing: false,
      self: false,
    },
  ]);
});
