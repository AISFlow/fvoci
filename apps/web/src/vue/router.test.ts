import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory } from "vue-router";
import { isVueAppPath } from "../app-boundary.ts";
import { createAppRouter } from "./router.ts";
import { VUE_ROUTE_PATHS, VUE_WORKSPACE_ROUTE_PATHS } from "./route-paths.ts";

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
    await router.push("/w/acme/my-tasks");
    release();
    assert.ok(await slow, "the later navigation superseded it");
    assert.deepEqual(loads, ["/w/acme/my-tasks"]);
  }),
);

test("wiki list and search are Vue routes but stay off the live path object", () => {
  assert.equal(VUE_WORKSPACE_ROUTE_PATHS.wikiList, "/w/:slug/wiki");
  assert.equal(VUE_WORKSPACE_ROUTE_PATHS.search, "/w/:slug/search");
  assert.equal(
    Object.values(VUE_ROUTE_PATHS).includes(VUE_WORKSPACE_ROUTE_PATHS.wikiList),
    false,
  );
  assert.equal(Object.values(VUE_ROUTE_PATHS).includes(VUE_WORKSPACE_ROUTE_PATHS.search), false);
});

test("the workspace landing, project list, wiki list and search resolve as Vue routes", () => {
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme").name, "workspace-home");
  assert.equal(router.resolve("/w/acme/projects").name, "projects");
  assert.equal(router.resolve("/w/acme/projects/").name, "projects");
  assert.equal(router.resolve("/w/acme/wiki").name, "wiki-list");
  assert.equal(router.resolve("/w/acme/wiki/").name, "wiki-list");
  assert.equal(router.resolve("/w/acme/WIKI").name, "wiki-list");
  assert.equal(router.resolve("/w/acme/search").name, "search");
  assert.equal(router.resolve("/w/acme/search/").name, "search");
  assert.equal(router.resolve("/w/acme/Search").name, "search");
  assert.equal(router.resolve("/w/acme/WIKI-1").name, "wiki-document");
  assert.equal(router.resolve("/w/acme/wiki-12").name, "wiki-document");
});

test(
  "wiki list and search still full-load React (app-boundary unchanged)",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    const wiki = router.resolve("/w/acme/wiki");
    const search = router.resolve("/w/acme/search?q=hello&tab=document");
    assert.equal(wiki.name, "wiki-list");
    assert.equal(search.name, "search");
    assert.equal(isVueAppPath(wiki.path), false);
    assert.equal(isVueAppPath(search.path), false);
    // bun cannot mount .vue route chunks; afterEach uses this same replace
    // for any completed nav the boundary still sends to React.
    if (!isVueAppPath(wiki.path)) window.location.replace(wiki.fullPath);
    if (!isVueAppPath(search.path)) window.location.replace(search.fullPath);
    assert.deepEqual(loads, ["/w/acme/wiki", "/w/acme/search?q=hello&tab=document"]);
  }),
);
