import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory, createRouter } from "vue-router";
import { isVueAppPath } from "./app-boundary.ts";
import { parseWikiRef } from "./lib/href.ts";
import { VUE_ROUTE_PATHS } from "./vue/route-paths.ts";

const SAMPLES = [
  "/w/acme/GNT/gantt",
  "/w/acme/GNT/gantt/",
  "/w/acme/gnt/gantt",
  "/w/acme/GNT/Gantt",
  "/w/acme/GNT/GANTT",
  "/W/acme/GNT/gantt",
  "/W/acme/GNT/Gantt/",
  "/w/acme/GNT/Tasks",
  "/w/acme/WIKI-3/gantt",
  "/w/acme/GNT",
  "/w/acme/GNT/tasks",
  "/w/acme/GNT/gantt/extra",
  "/w/acme/gantt",
  "/w/acme/GNT/ganttx",
  "/w//GNT/gantt",
  "/x/acme/GNT/gantt",
  "/",
  "/login",
  "/login/",
  "/setup",
  "/setup/",
  "/w/acme",
  "/w/acme/a/123/view",
  "/w/acme/WIKI-1",
  "/w/acme/WIKI-12/",
  "/w/acme/wiki-7",
  "/W/acme/Wiki-123456789",
  "/w/acme/WIKI-1234567890",
  "/w/acme/WIKI-0",
  "/w/acme/WIKI-01",
  "/w/acme/WIKI-",
  "/w/acme/WIKI-1a",
  "/w/acme/XWIKI-1",
  "/w/acme/WIKI-WIKI-1",
  "/w/acme/WIKI-1/extra",
  "/w/acme/wiki",
  "/w/acme/PRJ-1",
  "/w//WIKI-1",
];

test("the boot module sends the Gantt path, and only it, to the Vue app", () => {
  assert.equal(isVueAppPath("/w/acme/GNT/gantt"), true);
  assert.equal(isVueAppPath("/w/acme/GNT/gantt/"), true);
  // React Router matched the Gantt route in any case; the boundary does too.
  assert.equal(isVueAppPath("/w/acme/GNT/Gantt"), true);
  assert.equal(isVueAppPath("/W/acme/GNT/GANTT"), true);
  assert.equal(isVueAppPath("/w/acme/GNT"), false);
  assert.equal(isVueAppPath("/w/acme/GNT/gantt/extra"), false);
  assert.equal(isVueAppPath("/w/acme/GNT/tasks"), false);
  assert.equal(isVueAppPath("/"), false);
});

test("the boot module sends wiki documents, and only them, to the Vue app", () => {
  assert.equal(isVueAppPath("/w/acme/WIKI-1"), true);
  assert.equal(isVueAppPath("/w/acme/wiki-12/"), true);
  assert.equal(isVueAppPath("/w/acme/WIKI-123456789"), true);
  // parseWikiRef refuses these; the React app handles them as before.
  assert.equal(isVueAppPath("/w/acme/WIKI-0"), false);
  assert.equal(isVueAppPath("/w/acme/WIKI-01"), false);
  assert.equal(isVueAppPath("/w/acme/WIKI-1234567890"), false);
  // Task and project-document refs stay React pages.
  assert.equal(isVueAppPath("/w/acme/PRJ-1"), false);
  assert.equal(isVueAppPath("/w/acme/XWIKI-1"), false);
  assert.equal(isVueAppPath("/w/acme/wiki"), false);
});

test("the boot module sends /login, and only that path, to the Vue app", () => {
  assert.equal(isVueAppPath("/login"), true);
  assert.equal(isVueAppPath("/login/"), true);
  assert.equal(isVueAppPath("/LOGIN"), true);
  assert.equal(isVueAppPath("/login/extra"), false);
  assert.equal(isVueAppPath("/logins"), false);
});

test("the boot module still sends /setup to the React app", () => {
  assert.equal(isVueAppPath("/setup"), false);
  assert.equal(isVueAppPath("/setup/"), false);
  assert.equal(isVueAppPath("/SETUP"), false);
  assert.equal(isVueAppPath("/setups"), false);
});

test("every wiki path the boundary sends parses as the React app's wiki ref", () => {
  for (const path of SAMPLES) {
    // React Router matches /w/:slug in any case, as the boundary does.
    const ref = /^\/w\/[^/]+\/([^/]+)\/?$/i.exec(path)?.[1];
    const wiki = ref ? parseWikiRef(ref) : null;
    const gantt = /\/gantt\/?$/i.test(path);
    const login = /^\/login\/?$/i.test(path);
    if (!gantt && !login) assert.equal(isVueAppPath(path), wiki !== null, path);
  }
});

test("the Vue router matches exactly the paths the boundary sends it", () => {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      ...Object.values(VUE_ROUTE_PATHS).map((path) => ({ path, component: {} })),
      // As in src/vue/router.ts: every other path is the React app's.
      { path: "/:pathMatch(.*)*", name: "react-app", component: {} },
    ],
  });
  for (const path of SAMPLES) {
    const matched = router.resolve(path).name !== "react-app";
    assert.equal(matched, isVueAppPath(path), path);
  }
});
