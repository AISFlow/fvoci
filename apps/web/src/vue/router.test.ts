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
    await router.push("/login?next=%2Fw%2Facme");
    assert.deepEqual(loads, ["/login?next=%2Fw%2Facme"]);
  }),
);

test("the public-share route is declared (boot still needs the boundary regex)", () => {
  assert.equal(
    routes.some((route) => route.name === "public-share" && route.path === "/s/:token"),
    true,
  );
  assert.equal(
    routes.some((route) => typeof route.path === "string" && route.path.includes("attachments")),
    false,
  );
});

test(
  "a completed navigation to /s/tok is a full page load (boot is still React)",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    await router.push("/s/tok");
    assert.deepEqual(loads, ["/s/tok"]);
  }),
);

test(
  "a completed navigation to the share attachment viewer is not this page",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    await router.push("/s/tok/attachments/att/view");
    assert.equal(router.currentRoute.value.name, "react-app");
    assert.deepEqual(loads, ["/s/tok/attachments/att/view"]);
  }),
);

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
