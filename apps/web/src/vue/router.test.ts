import assert from "node:assert/strict";
import test from "node:test";
import { isVueAppPath } from "@/app-boundary";
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
    await router.push("/w/acme");
    assert.deepEqual(loads, ["/w/acme"]);
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
  assert.equal(isVueAppPath("/setups"), false);
});

test(
  "setup, home, invite and public navigation stay Vue; admin legal leaves with its query and fragment",
  withLocation(async (loads) => {
    const router = createAppRouter(createMemoryHistory());
    // Bun does not compile SFCs. Exercise the real router/afterEach with
    // inert pages; mounted page behavior belongs to the browser groups.
    for (const route of routes) {
      router.removeRoute(route.name!);
      router.addRoute({ path: route.path, name: route.name, component: { render: () => null } });
    }
    for (const path of ["/setup", "/", "/invite/tok", "/legal/terms?version=1", "/service-info"]) {
      await router.push(path);
      assert.deepEqual(loads, [], path);
    }
    await router.push("/settings/legal?kind=terms#editor");
    assert.deepEqual(loads, ["/settings/legal?kind=terms#editor"]);
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
