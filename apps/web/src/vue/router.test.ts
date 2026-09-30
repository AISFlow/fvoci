import assert from "node:assert/strict";
import test from "node:test";
import { isVueAppPath } from "@/app-boundary";
import { createMemoryHistory } from "vue-router";
import { createAppRouter, routes } from "./router.ts";
import { VUE_ROUTE_PATHS, VUE_WORKSPACE_ROUTE_PATHS, VUE_NAV_ROUTE_PATHS } from "./route-paths.ts";

// Unknown paths follow Vue redirects; failed or superseded navigation never loads a document.
const routerName = (path: string) => createAppRouter(createMemoryHistory()).resolve(path).name;

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
  "unknown nested paths replace with home without a full load",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    router.getRoutes().find(record => record.name === "home")!.components = { default: { render: () => null } };
    await router.push("/settings/account/extra");
    assert.equal(router.currentRoute.value.path, "/");
    assert.deepEqual(loads, []);
  }),
);

test(
  "a completed navigation to /setup stays in the Vue app",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    await router.push("/setup?next=%2Fw%2Facme");
    assert.deepEqual(loads, []);
  }),
);

test(
  "a completed navigation to /login stays in the Vue app",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    await router.push("/login");
    assert.deepEqual(loads, []);
  }),
);

test("the login route is declared (the boundary regex sends /login to Vue)", () => {
  assert.equal(
    routes.some((route) => route.name === "login" && route.path === "/login"),
    true,
  );
});

test("home, legal, and service-info are declared live Vue paths", () => {
  assert.equal(
    routes.some((route) => route.name === "home" && route.path === "/"),
    true,
  );
  assert.equal(
    routes.some((route) => route.name === "legal" && route.path === "/legal/:kind"),
    true,
  );
  assert.equal(
    routes.some((route) => route.name === "service-info" && route.path === "/service-info"),
    true,
  );
});

test("the invite route is declared (the boundary sends token paths to Vue)", () => {
  assert.equal(
    routes.some((route) => route.name === "invite" && route.path === "/invite/:token"),
    true,
  );
});

test(
  "a completed navigation to an invite stays in the Vue app",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    await router.push("/invite/tok");
    assert.deepEqual(loads, []);
  }),
);

test("the setup route is declared and the boundary sends /setup to Vue", () => {
  assert.equal(
    routes.some((route) => route.name === "setup" && route.path === "/setup"),
    true,
  );
  assert.equal(isVueAppPath("/setup"), true);
  assert.equal(isVueAppPath("/setup/"), true);
  assert.equal(isVueAppPath("/SETUP"), true);
  assert.equal(routerName("/setups"), "unknown-path");
});

test(
  "setup, home, invite, public and account/admin navigation stay Vue with query and fragment",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    // Bun does not compile SFCs. Exercise the real router/afterEach with
    // inert pages; mounted page behavior belongs to the browser groups.
    for (const route of routes) {
      router.removeRoute(route.name!);
      router.addRoute({ path: route.path, name: route.name, component: { render: () => null } });
    }
    for (const path of ["/setup", "/", "/invite/tok", "/legal/terms?version=1", "/service-info", "/s/tok?search=hello#reader", "/settings/account", "/settings/admin", "/settings/audit"]) {
      await router.push(path);
      assert.deepEqual(loads, [], path);
    }
    await router.push("/settings/legal?kind=terms#editor");
    assert.deepEqual(loads, []);
  }),
);

test(
  "fallback navigation still honors aborted and superseded page guards",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    for (const record of router.getRoutes()) if (record.components) record.components = { default: { render: () => null } };
    await router.push("/w/acme/projects");
    let release = () => {};
    let entered = () => {};
    const waiting = new Promise<void>(resolve => { entered = resolve; });
    router.beforeEach(to => {
      if (to.path === "/") return false;
      if (to.path === "/w/acme/slow!") { entered(); return new Promise<void>(resolve => { release = resolve; }); }
      return true;
    });
    assert.ok(await router.push("/unknown/nested"));
    assert.equal(router.currentRoute.value.path, "/w/acme/projects");
    const slow = router.push("/w/acme/slow!");
    await waiting;
    await router.push("/w/acme/wiki");
    release();
    assert.ok(await slow);
    assert.equal(router.currentRoute.value.path, "/w/acme/wiki");
    assert.deepEqual(loads, []);
  }),
);

test("wiki list and search are live Vue routes", () => {
  assert.equal(VUE_WORKSPACE_ROUTE_PATHS.wikiList, "/w/:slug/wiki");
  assert.equal(VUE_WORKSPACE_ROUTE_PATHS.search, "/w/:slug/search");
  assert.equal(
    Object.values(VUE_ROUTE_PATHS).includes(VUE_WORKSPACE_ROUTE_PATHS.wikiList),
    true,
  );
  assert.equal(Object.values(VUE_ROUTE_PATHS).includes(VUE_WORKSPACE_ROUTE_PATHS.search), true);
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
  "wiki list and search stay Vue",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    const wiki = router.resolve("/w/acme/wiki");
    const search = router.resolve("/w/acme/search?q=hello&tab=document");
    assert.equal(wiki.name, "wiki-list");
    assert.equal(search.name, "search");
    assert.equal(isVueAppPath(wiki.path), true);
    assert.equal(isVueAppPath(search.path), true);
    // bun cannot mount .vue route chunks; afterEach uses this same replace
    // for any completed nav the boundary still sends to React.
    if (!isVueAppPath(wiki.path)) window.location.replace(wiki.fullPath);
    if (!isVueAppPath(search.path)) window.location.replace(search.fullPath);
    assert.deepEqual(loads, []);
  }),
);

test("my-tasks, notifications, and trash are live Vue paths", () => {
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
  assert.equal(isVueAppPath("/w/acme/my-tasks"), true);
  assert.equal(isVueAppPath("/w/acme/notifications"), true);
  assert.equal(isVueAppPath("/w/acme/trash"), true);
});

test(
  "navigating to my-tasks, notifications, or trash stays Vue",
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
    assert.deepEqual(loads, []);
  }),
);

const AUTH_REST = [
  { name: "reset-password", path: "/reset-password" },
  { name: "magic-link", path: "/magic-link" },
  { name: "confirm-email", path: "/confirm-email" },
  { name: "cancel-withdraw", path: "/cancel-withdraw" },
  { name: "consent", path: "/consent" },
] as const;

test("the remaining auth routes are declared live Vue paths", () => {
  for (const { name, path } of AUTH_REST) {
    assert.equal(
      routes.some((route) => route.name === name && route.path === path),
      true,
      name,
    );
    // The boundary and router agree for case and trailing slash variants.
    assert.equal(isVueAppPath(path), true, path);
    assert.equal(isVueAppPath(`${path}/`), true, `${path}/`);
    assert.equal(isVueAppPath(path.toUpperCase()), true, path.toUpperCase());
  }
});

test(
  "a completed navigation to a remaining auth page stays in Vue",
  withLocation(async (loads) => {
    for (const path of [
      "/reset-password?token=tok",
      "/magic-link?token=tok",
      "/confirm-email?token=tok",
      "/cancel-withdraw",
      "/consent?returnTo=%2F",
    ]) {
      const router = createAppRouter(createMemoryHistory());
      for (const route of routes) {
        router.removeRoute(route.name!);
        router.addRoute({ path: route.path, name: route.name, component: { render: () => null } });
      }
      await router.push(path);
      assert.deepEqual(loads.splice(0), [], path);
    }
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
  // These resource routes now stay within Vue.
  assert.equal(isVueAppPath("/w/acme/wiki-3"), true);
  assert.equal(isVueAppPath("/w/acme/GNT"), true);
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
  // These resource routes now stay within Vue.
  assert.equal(isVueAppPath("/w/acme/GNT-1"), true);
  assert.equal(isVueAppPath("/w/acme/wiki-3"), true);
});

test("workspace settings routes are live Vue paths", () => {
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme/settings").name, "workspace-settings");
  assert.equal(router.resolve("/w/acme/settings/document-tags").name, "workspace-settings-document-tags");
  assert.equal(router.resolve("/w/acme/settings/templates").name, "workspace-settings-templates");
  // Boot and router agree on these exact settings paths.
  assert.equal(isVueAppPath("/w/acme/settings"), true);
  assert.equal(isVueAppPath("/w/acme/settings/document-tags"), true);
  assert.equal(isVueAppPath("/w/acme/settings/templates"), true);
});

test("project fields and workflow settings resolve to their own lazy pages", () => {
  const router = createAppRouter(createMemoryHistory());
  assert.equal(router.resolve("/w/acme/GNT/settings/fields").name, "project-fields");
  assert.equal(router.resolve("/w/acme/GNT/settings/workflow").name, "project-workflow");
});

test("decoded and invalid single-segment refs reach the guarded fallback, exact sections retain precedence", () => {
  const router = createAppRouter(createMemoryHistory());
  for (const path of ["/w/acme/%47NT", "/w/acme/%20GNT%20", "/w/acme/%EF%BC%A7%EF%BC%AE%EF%BC%B4", "/w/acme/WIKI-01", "/w/acme/a", "/w/acme/bad!"]) assert.equal(router.resolve(path).name, "workspace-ref", path);
  assert.equal(router.resolve("/w/acme/%47NT").params.ref, "GNT");
  for (const path of ["/w/acme/wiki/extra", "/settings/account/extra"]) assert.equal(router.resolve(path).name, "unknown-path", path);
});

test("canonical ref replacement preserves the current query spelling and hash; edits serialize normally", async () => {
  const router = createAppRouter(createMemoryHistory());
  for (const record of router.getRoutes()) if (record.components) record.components = { default: { render: () => null } };
  await router.push("/w/acme/%47NT?from=encoded%20ref&x=1&x=2#overview");
  const current = router.currentRoute.value;
  await router.replace({ path: "/w/acme/GNT", query: current.query, hash: current.hash });
  assert.equal(router.currentRoute.value.fullPath, "/w/acme/GNT?from=encoded%20ref&x=1&x=2#overview");
  await router.replace({ path: "/w/acme/GNT", query: { from: "new value" } });
  assert.equal(router.currentRoute.value.fullPath, "/w/acme/GNT?from=new+value");
});
