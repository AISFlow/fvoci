import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory, createRouter } from "vue-router";
import { followAppHref } from "./navigation";

await test("a rejected local route chunk reaches the browser error owner without a document navigation", async () => {
  const failure = new Error("route chunk failed");
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: "/broken", component: () => Promise.reject(failure) }],
  });
  router.onError((error) => {
    assert.equal(error, failure);
  });
  const previous = Object.getOwnPropertyDescriptor(globalThis, "reportError");
  let report: ((error: unknown) => void) | undefined;
  const reported = new Promise<unknown>((resolve) => {
    report = resolve;
  });
  Object.defineProperty(globalThis, "reportError", {
    configurable: true,
    value: (error: unknown) => {
      assert.ok(report);
      report(error);
    },
  });
  try {
    followAppHref("/broken?from=shell#section", router);
    assert.equal(await reported, failure);
    assert.equal(router.currentRoute.value.path, "/");
  } finally {
    if (previous) Object.defineProperty(globalThis, "reportError", previous);
    else Reflect.deleteProperty(globalThis, "reportError");
  }
});
