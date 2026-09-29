import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory, createRouter } from "vue-router";
import { isVueAppPath } from "./app-boundary.ts";
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
  "/w/acme",
  "/w/acme/a/123/view",
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
