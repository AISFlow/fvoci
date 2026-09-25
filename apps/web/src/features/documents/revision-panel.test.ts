import assert from "node:assert/strict";
import { test } from "node:test";
import { persistThenCreate } from "./revision-persist.ts";

test("manual save calls persistNow first", async () => {
  const order: string[] = [];
  await persistThenCreate(
    () => {
      order.push("persist");
    },
    () => {
      order.push("create");
    },
  );
  assert.deepEqual(order, ["persist", "create"]);
});

test("failed persistence prevents creating a revision", async () => {
  let created = false;
  await assert.rejects(
    persistThenCreate(
      () => Promise.reject(new Error("save failed")),
      () => {
        created = true;
      },
    ),
    /save failed/,
  );
  assert.equal(created, false);
});
