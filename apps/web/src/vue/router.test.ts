import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory } from "vue-router";
import { createAppRouter, routes } from "./router.ts";

// A navigation to a React page leaves the Vue app with a full load; one that
// failed or was superseded never happened and loads nothing.

function withLocation(run: (loads: string[]) => Promise<void>): () => Promise<void> {
  return async () => {
    const loads: string[] = [];
    const previous = (globalThis as { window?: unknown }).window;
    (globalThis as { window?: unknown }).window = { location: { replace: (url: string) => loads.push(url) } };
    try {
      await run(loads);
    } finally {
      (globalThis as { window?: unknown }).window = previous;
    }
  };
}

test(
  "a completed navigation to a React page is a full page load",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    await router.push("/setup?next=%2Fw%2Facme");
    assert.deepEqual(loads, ["/setup?next=%2Fw%2Facme"]);
  }),
);

test("the login route is declared (boot still needs the boundary regex)", () => {
  assert.equal(
    routes.some((route) => route.name === "login" && route.path === "/login"),
    true,
  );
});

test(
  "a failed or superseded navigation to a React page loads nothing",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    let release: () => void = () => undefined;
    router.beforeEach((to) => {
      if (to.path === "/blocked") return false;
      if (to.path === "/slow") return new Promise<void>((resolve) => (release = resolve));
      return true;
    });

    const blocked = await router.push("/blocked");
    assert.ok(blocked, "the guard aborted the navigation");
    assert.deepEqual(loads, []);

    const slow = router.push("/slow");
    await router.push("/w/acme");
    release();
    assert.ok(await slow, "the later navigation superseded it");
    assert.deepEqual(loads, ["/w/acme"]);
  }),
);
