import assert from "node:assert/strict";
import test from "node:test";
import { tProblemTitle } from "@fvoci/i18n";
import * as api from "./api.ts";

type Problem = { code: string; params?: unknown };

async function problemOf(status: number, body: Problem): Promise<api.ProblemError> {
  const result = {
    error: { type: "about:blank", title: "x", status, ...body },
    response: new Response(null, { status }),
  };
  try {
    await api.ensureOk(result);
  } catch (err) {
    assert.ok(err instanceof api.ProblemError);
    return err;
  }
  assert.fail("ensureOk must throw for a problem body");
}

// The three shapes a stale or foreign cursor has on the wire today.
const INVALID_CURSOR_SHAPES: Array<[string, Problem]> = [
  // collections, task lists, search, notifications, events, admin, projects
  ["invalid_input with params.code", { code: "invalid_input", params: { code: "invalid_cursor" } }],
  // comments
  ["invalid_cursor with params", { code: "invalid_cursor", params: { code: "invalid_cursor" } }],
  // task activity
  ["invalid_cursor without params", { code: "invalid_cursor" }],
];

await test("ensureOk keeps the specific reason under a general problem code", async () => {
  const err = await problemOf(400, INVALID_CURSOR_SHAPES[0][1]);
  assert.equal(err.status, 400);
  assert.equal(err.code, "invalid_input");
  assert.equal(err.reason, "invalid_cursor");
  assert.equal(err.title, tProblemTitle("invalid_input"), "the title still follows the code");
});

await test("ensureOk ignores params without a string code", async () => {
  for (const params of [undefined, null, "invalid_cursor", { code: 5 }, { field: "x" }]) {
    const err = await problemOf(400, { code: "invalid_input", params });
    assert.equal(err.reason, undefined, JSON.stringify(params));
  }
});

for (const [shape, body] of INVALID_CURSOR_SHAPES) {
  await test(`isInvalidCursor accepts ${shape}`, async () => {
    const err = await problemOf(400, body);
    assert.equal(api.isInvalidCursor(err), true);
    assert.equal(api.isInvalidInput(err), false, "a stale cursor is not an invalid query");
  });
}

await test("a genuinely invalid query is invalid input, not a stale cursor", async () => {
  const err = await problemOf(400, { code: "invalid_input", params: { field: "config" } });
  assert.equal(api.isInvalidCursor(err), false);
  assert.equal(api.isInvalidInput(err), true);
});

await test("non-problem errors are neither", () => {
  for (const err of [new Error("network"), null, undefined, new api.ProblemError(500)]) {
    assert.equal(api.isInvalidCursor(err), false);
    assert.equal(api.isInvalidInput(err), false);
  }
});

await test("ensureOk returns rejected promises without throwing synchronously", async () => {
  const missing = api.ensureOk({ response: new Response(null, { status: 200 }) });
  assert.ok(missing instanceof Promise);
  await assert.rejects(
    missing,
    (error: unknown) => error instanceof api.ProblemError && error.status === 500,
  );
  const forbidden = api.ensureOk({
    error: { type: "about:blank", title: "Forbidden", status: 403, code: "forbidden" },
    response: new Response(null, { status: 403 }),
  });
  assert.ok(forbidden instanceof Promise);
  await assert.rejects(
    forbidden,
    (error: unknown) =>
      error instanceof api.ProblemError && error.status === 403 && error.code === "forbidden",
  );
  assert.deepEqual(
    await api.ensureOk({ data: { id: "saved" }, response: new Response(null, { status: 200 }) }),
    { id: "saved" },
  );
});

await test("ensureOk also rejects unexpected exceptions during result evaluation", async () => {
  const failure = new Error("result getter failed");
  const result = api.ensureOk({
    get data(): never {
      throw failure;
    },
    response: new Response(null, { status: 200 }),
  });
  assert.ok(result instanceof Promise);
  await assert.rejects(result, failure);
});
