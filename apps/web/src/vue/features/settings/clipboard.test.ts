import assert from "node:assert/strict";
import test from "node:test";
import { copyText } from "./clipboard.ts";

test("copyText writes to the clipboard and fails without one", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const written: string[] = [];
  try {
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value: { clipboard: { writeText: async (value: string) => void written.push(value) } },
    });
    await copyText("https://example.test/invite");
    assert.deepEqual(written, ["https://example.test/invite"]);
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: {} });
    await assert.rejects(copyText("x"), /clipboard unavailable/);
  } finally {
    if (original) Object.defineProperty(globalThis, "navigator", original);
    else delete (globalThis as { navigator?: unknown }).navigator;
  }
});
