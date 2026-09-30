import assert from "node:assert/strict";
import test from "node:test";
import { IMPORT_POLL_MS, IMPORT_POLL_TRIES, pollImportJob } from "./import-poll.ts";

const noSleep = (_ms: number, signal?: AbortSignal): Promise<"cancelled" | "ok"> =>
  Promise.resolve(signal?.aborted ? "cancelled" : "ok");

await test("poll budget matches the source (1500 ms x 40)", () => {
  assert.equal(IMPORT_POLL_MS, 1500);
  assert.equal(IMPORT_POLL_TRIES, 40);
});

await test("poll stops at the first terminal status", async () => {
  const seen: string[] = ["running", "running", "completed"];
  let calls = 0;
  const outcome = await pollImportJob({
    fetchStatus: () => Promise.resolve({ status: seen[calls++] ?? "running" }),
    intervalMs: 1,
    maxTries: 10,
    sleep: noSleep,
  });
  assert.deepEqual(outcome, { kind: "completed" });
  assert.equal(calls, 3);
});

await test("failed job and spent budget are distinct outcomes", async () => {
  assert.deepEqual(
    await pollImportJob({
      fetchStatus: () => Promise.resolve({ status: "failed" }),
      intervalMs: 1,
      maxTries: 3,
      sleep: noSleep,
    }),
    { kind: "failed" },
  );
  let calls = 0;
  assert.deepEqual(
    await pollImportJob({
      fetchStatus: () => {
        calls += 1;
        return Promise.resolve({ status: "running" });
      },
      intervalMs: 1,
      maxTries: 3,
      sleep: noSleep,
    }),
    { kind: "budget" },
  );
  assert.equal(calls, 3);
});

await test("abort during the wait cancels without another fetch", async () => {
  const controller = new AbortController();
  let calls = 0;
  const outcome = await pollImportJob({
    fetchStatus: () => {
      calls += 1;
      controller.abort();
      return Promise.resolve({ status: "running" });
    },
    signal: controller.signal,
    intervalMs: 60_000,
    maxTries: 5,
  });
  assert.deepEqual(outcome, { kind: "cancelled" });
  assert.equal(calls, 1);
});
