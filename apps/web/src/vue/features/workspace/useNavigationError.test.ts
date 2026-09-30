import assert from "node:assert/strict";
import test from "node:test";
import { t } from "@fvoci/i18n";
import { effectScope, ref } from "vue";
import { useNavigationError } from "./useNavigationError";

function rejection() {
  let reject: ((failure: Error) => void) | undefined;
  const promise = new Promise<void>((_resolve, fail) => {
    reject = fail;
  });
  assert.ok(reject, "a controlled rejection must be initialized");
  return { promise, reject };
}
await test("navigation failures are visible only to their current page and attempt", async () => {
  for (const retirement of ["current", "aba", "superseded", "dispose"]) {
    const scope = effectScope();
    const page = ref("/w/alpha/wiki");
    const navigation = scope.run(() => useNavigationError(() => page.value));
    assert.ok(navigation);
    try {
      const old = rejection();
      navigation.run(() => old.promise);
      if (retirement === "aba") {
        page.value = "/w/beta/wiki";
        page.value = "/w/alpha/wiki";
      }
      if (retirement === "dispose") scope.stop();
      const current = retirement === "superseded" ? rejection() : undefined;
      if (current) navigation.run(() => current.promise);
      old.reject(new Error("obsolete navigation"));
      await Promise.resolve();
      assert.equal(navigation.error.value, retirement === "current" ? t("load.failed") : null);
      if (current) {
        current.reject(new Error("current navigation"));
        await Promise.resolve();
        assert.equal(navigation.error.value, t("load.failed"));
      }
    } finally {
      scope.stop();
    }
  }
});
await test("a new navigation clears the preceding failure and keeps its returned rejection handled", async () => {
  const scope = effectScope();
  const navigation = scope.run(() => useNavigationError(() => "/"));
  assert.ok(navigation);
  try {
    navigation.run(() => Promise.reject(new Error("first")));
    await Promise.resolve();
    assert.equal(navigation.error.value, t("load.failed"));
    navigation.run(() => Promise.resolve());
    assert.equal(navigation.error.value, null);
    await Promise.resolve();
    assert.equal(navigation.error.value, null);
  } finally {
    scope.stop();
  }
});
