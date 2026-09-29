import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory } from "vue-router";
import { isVueAppPath } from "@/app-boundary";
import { createAppRouter, routes } from "./router.ts";
import { VUE_NAV_ROUTE_PATHS } from "./route-paths.ts";

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
    await router.push("/w/acme/search");
    release();
    assert.ok(await slow, "the later navigation superseded it");
    assert.deepEqual(loads, ["/w/acme/search"]);
  }),
);

test("the workspace landing and project list resolve as Vue routes", () => {
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme").name, "workspace-home");
  assert.equal(router.resolve("/w/acme/projects").name, "projects");
  assert.equal(router.resolve("/w/acme/projects/").name, "projects");
  assert.equal(router.resolve("/w/acme/wiki").name, "react-app");
  assert.equal(router.resolve("/w/acme/WIKI-1").name, "wiki-document");
});

test("my-tasks, notifications, and trash are declared but not live Vue paths", () => {
  assert.equal(
    routes.some((route) => route.name === "my-tasks" && route.path === VUE_NAV_ROUTE_PATHS.myTasks),
    true,
  );
  assert.equal(
    routes.some((route) => route.name === "notifications" && route.path === VUE_NAV_ROUTE_PATHS.notifications),
    true,
  );
  assert.equal(
    routes.some((route) => route.name === "trash" && route.path === VUE_NAV_ROUTE_PATHS.trash),
    true,
  );
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme/my-tasks").name, "my-tasks");
  assert.equal(router.resolve("/w/acme/my-tasks/").name, "my-tasks");
  assert.equal(router.resolve("/w/acme/notifications").name, "notifications");
  assert.equal(router.resolve("/w/acme/trash").name, "trash");
  assert.equal(isVueAppPath("/w/acme/my-tasks"), false);
  assert.equal(isVueAppPath("/w/acme/notifications"), false);
  assert.equal(isVueAppPath("/w/acme/trash"), false);
});

test(
  "navigating to my-tasks, notifications, or trash is a full page load (boot is still React)",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    // bun test does not compile .vue lazy chunks; the afterEach guard is
    // what we need, and it runs after a completed navigation.
    const dummy = { render: () => null };
    for (const record of router.getRoutes()) {
      if (record.name === "my-tasks" || record.name === "notifications" || record.name === "trash") {
        record.components = { default: dummy };
      }
    }
    await router.push("/w/acme/my-tasks");
    await router.push("/w/acme/notifications");
    await router.push("/w/acme/trash");
    assert.deepEqual(loads, ["/w/acme/my-tasks", "/w/acme/notifications", "/w/acme/trash"]);
  }),
);
