import assert from "node:assert/strict";
import test from "node:test";
import { IMPORT_POLL_MS, IMPORT_POLL_TRIES, pollImportJob } from "./import-poll.ts";

const noSleep = async (_ms: number, signal?: AbortSignal) =>
  signal?.aborted ? ("cancelled" as const) : ("ok" as const);

test("poll budget matches the source (1500 ms x 40)", () => {
  assert.equal(IMPORT_POLL_MS, 1500);
  assert.equal(IMPORT_POLL_TRIES, 40);
});

test("poll stops at the first terminal status", async () => {
  const seen: string[] = ["running", "running", "completed"];
  let calls = 0;
  const outcome = await pollImportJob({
    fetchStatus: async () => ({ status: seen[calls++] ?? "running" }),
    intervalMs: 1,
    maxTries: 10,
    sleep: noSleep,
  });
  assert.deepEqual(outcome, { kind: "completed" });
  assert.equal(calls, 3);
});

test("failed job and spent budget are distinct outcomes", async () => {
  assert.deepEqual(
    await pollImportJob({ fetchStatus: async () => ({ status: "failed" }), intervalMs: 1, maxTries: 3, sleep: noSleep }),
    { kind: "failed" },
  );
  let calls = 0;
  assert.deepEqual(
    await pollImportJob({
      fetchStatus: async () => {
        calls += 1;
        return { status: "running" };
      },
      intervalMs: 1,
      maxTries: 3,
      sleep: noSleep,
    }),
    { kind: "budget" },
  );
  assert.equal(calls, 3);
});

test("abort during the wait cancels without another fetch", async () => {
  const controller = new AbortController();
  let calls = 0;
  const outcome = await pollImportJob({
    fetchStatus: async () => {
      calls += 1;
      controller.abort();
      return { status: "running" };
    },
    signal: controller.signal,
    intervalMs: 60_000,
    maxTries: 5,
  });
  assert.deepEqual(outcome, { kind: "cancelled" });
  assert.equal(calls, 1);
});
