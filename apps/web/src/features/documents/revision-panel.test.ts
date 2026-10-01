import assert from "node:assert/strict";
import { test } from "node:test";
import { persistThenCreate } from "./revision-persist.ts";

await test("manual save calls persistNow first", async () => {
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

await test("failed persistence prevents creating a revision", async () => {
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

await test("unavailable persist barrier fails explicitly instead of creating stale history", async () => {
  let created = false;
  await assert.rejects(
    persistThenCreate(undefined, () => {
      created = true;
    }),
    /live persist barrier/,
  );
  assert.equal(created, false);
});

await test("persistThenCreate stays pending until create completes and propagates its failure", async () => {
  let reject!: (error: Error) => void;
  const creating = new Promise<void>((_resolve, no) => {
    reject = no;
  });
  let settled = false;
  const saving = persistThenCreate(
    () => Promise.resolve(),
    () => creating,
  );
  void saving
    .finally(() => {
      settled = true;
    })
    .catch(() => undefined);
  await Promise.resolve();
  assert.equal(settled, false);
  reject(new Error("create failed"));
  await assert.rejects(saving, /create failed/);
});
