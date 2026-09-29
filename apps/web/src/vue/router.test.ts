import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import { createAppRouter } from "./router.ts";

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

test("wiki documents stay wiki; project keys are project-home; gantt stays gantt", () => {
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme/wiki-3").name, "wiki-document");
  assert.equal(router.resolve("/w/acme/WIKI-3").name, "wiki-document");
  assert.equal(router.resolve("/w/acme/GNT").name, "project-home");
  assert.equal(router.resolve("/w/acme/gnt").name, "project-home");
  assert.equal(router.resolve("/w/acme/GNT/gantt").name, "project-gantt");
  assert.equal(router.resolve("/w/acme/GNT/tasks").name, "project-tasks");
  assert.equal(router.resolve("/w/acme/GNT/board").name, "project-board");
  // Declared but not live: afterEach still full-loads (same as /login above).
  assert.equal(isVueAppPath("/w/acme/wiki-3"), true);
  assert.equal(isVueAppPath("/w/acme/GNT"), false);
  assert.equal(isVueAppPath("/w/acme/GNT/gantt"), true);
});

test("workspace-item is more specific than project-home; wiki stays wiki", () => {
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme/GNT-1").name, "workspace-item");
  assert.equal(router.resolve("/w/acme/gnt-12").name, "workspace-item");
  assert.equal(router.resolve("/w/acme/wiki-3").name, "wiki-document");
  assert.equal(router.resolve("/w/acme/WIKI-3").name, "wiki-document");
  assert.equal(router.resolve("/w/acme/GNT").name, "project-home");
  assert.equal(router.resolve("/w/acme/GNT/tasks").name, "project-tasks");
  // Declared but not live: afterEach still full-loads (same as /login above).
  assert.equal(isVueAppPath("/w/acme/GNT-1"), false);
  assert.equal(isVueAppPath("/w/acme/wiki-3"), true);
});
