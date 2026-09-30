import assert from "node:assert/strict";
import test from "node:test";
import {
  aiInsertNodes,
  appendRange,
  applyTaskTitles,
  hasPendingTask,
  isDefiniteStatus,
  type TaskApplyState,
} from "./document-ai-apply.ts";

await test("요약은 줄마다 문단, 링크 제안은 문서 멘션 한 문단이다", () => {
  assert.deepEqual(aiInsertNodes({ action: "summarize", lines: ["첫 줄 🙂", "", "둘째"] }), [
    { type: "paragraph", content: [{ type: "text", text: "첫 줄 🙂" }] },
    { type: "paragraph", content: [{ type: "text", text: "둘째" }] },
  ]);
  assert.deepEqual(
    aiInsertNodes({
      action: "suggestLinks",
      links: [
        { id: "a", label: "가" },
        { id: "b", label: "나" },
      ],
    }),
    [
      {
        type: "paragraph",
        content: [
          { type: "mention", attrs: { entity: "document", id: "a", label: "가" } },
          { type: "text", text: " " },
          { type: "mention", attrs: { entity: "document", id: "b", label: "나" } },
        ],
      },
    ],
  );
  assert.deepEqual(aiInsertNodes({ action: "suggestLinks", links: [] }), []);
});

class Rejected extends Error {
  readonly status: number;
  constructor(status: number) {
    super(`status ${String(status)}`);
    this.status = status;
  }
}
const definite = (error: unknown) =>
  isDefiniteStatus(error instanceof Rejected ? error.status : null);

await test("부분 실패 후 재시도는 이미 만든 태스크를 다시 만들지 않는다", async () => {
  const sent: string[] = [];
  let failB = true;
  const create = (title: string) => {
    sent.push(title);
    if (title === "B" && failB) return Promise.reject(new Rejected(422));

    return Promise.resolve();
  };
  const first = await applyTaskTitles(["A", "B", "C"], [], create, definite);
  assert.deepEqual(first.states, ["created", "pending", "pending"]);
  assert.equal(first.created, 1);
  assert.equal(first.failure?.kind, "definite");
  assert.equal(hasPendingTask(first.states), true);

  failB = false;
  const second = await applyTaskTitles(["A", "B", "C"], first.states, create, definite);
  assert.deepEqual(second.states, ["created", "created", "created"]);
  assert.equal(second.created, 2);
  assert.equal(second.failure, null);
  assert.deepEqual(sent, ["A", "B", "B", "C"]);
  assert.equal(hasPendingTask(second.states), false);
});

await test("응답이 없거나 5xx 면 커밋됐을 수 있으니 재시도 대상에서 뺀다", async () => {
  const sent: string[] = [];
  const create = (title: string) => {
    sent.push(title);
    if (title === "A") return Promise.reject(new TypeError("network"));
    if (title === "B") return Promise.reject(new Rejected(502));

    return Promise.resolve();
  };
  const first = await applyTaskTitles(["A", "B"], [], create, definite);
  assert.deepEqual(first.states, ["unknown", "pending"]);
  assert.equal(first.failure?.kind, "unknown");
  const second = await applyTaskTitles(["A", "B"], first.states, create, definite);
  assert.deepEqual(second.states, ["unknown", "unknown"]);
  assert.deepEqual(sent, ["A", "B"]);
  const states: TaskApplyState[] = second.states;
  assert.equal(hasPendingTask(states), false);
});

await test("진행 콜백은 성공·실패마다 현재 상태 사본을 받는다", async () => {
  const seen: TaskApplyState[][] = [];
  await applyTaskTitles(
    ["A", "B"],
    [],
    (title) => {
      if (title === "B") return Promise.reject(new Rejected(403));

      return Promise.resolve();
    },
    definite,
    (states) => seen.push(states),
  );
  assert.deepEqual(seen, [
    ["created", "pending"],
    ["created", "pending"],
  ]);
});

await test("4xx 만 확정 거절이다", () => {
  assert.equal(isDefiniteStatus(400), true);
  assert.equal(isDefiniteStatus(409), true);
  assert.equal(isDefiniteStatus(500), false);
  assert.equal(isDefiniteStatus(503), false);
  assert.equal(isDefiniteStatus(null), false);
});

const block = (name: string, contentSize: number) => ({
  type: { name },
  content: { size: contentSize },
  nodeSize: contentSize + 2,
});

await test("추가 위치는 마지막 블록 뒤이고, 빈 마지막 문단만 대체한다", () => {
  // "기존 본문" paragraph: text stays whole, the result goes after it.
  assert.deepEqual(appendRange({ content: { size: 7 }, lastChild: block("paragraph", 5) }), {
    from: 7,
    to: 7,
  });
  // A list or code block at the end is not entered.
  assert.deepEqual(appendRange({ content: { size: 20 }, lastChild: block("bulletList", 18) }), {
    from: 20,
    to: 20,
  });
  assert.deepEqual(appendRange({ content: { size: 12 }, lastChild: block("codeBlock", 10) }), {
    from: 12,
    to: 12,
  });
  // Trailing empty paragraph (size 2) is replaced, not left as a blank line.
  assert.deepEqual(appendRange({ content: { size: 9 }, lastChild: block("paragraph", 0) }), {
    from: 7,
    to: 9,
  });
  assert.deepEqual(appendRange({ content: { size: 0 }, lastChild: null }), { from: 0, to: 0 });
});
