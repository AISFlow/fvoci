import assert from "node:assert/strict";
import test from "node:test";
import { createMemoryHistory, createRouter } from "vue-router";
import { isVueAppPath } from "./app-boundary.ts";
import { parseRef } from "./lib/href.ts";
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
  "/reset-password",
  "/reset-password/",
  "/RESET-PASSWORD",
  "/magic-link",
  "/confirm-email",
  "/cancel-withdraw",
  "/consent",
  "/reset-password/",
  "/RESET-PASSWORD",
  "/reset-password/extra",
  "/reset-passwords",
  "/magic-link/",
  "/MAGIC-LINK",
  "/magic-link/extra",
  "/magic-links",
  "/confirm-email/",
  "/CONFIRM-EMAIL",
  "/confirm-email/extra",
  "/confirm-emails",
  "/cancel-withdraw/",
  "/CANCEL-WITHDRAW",
  "/cancel-withdraw/extra",
  "/cancel-withdraws",
  "/consent/",
  "/CONSENT",
  "/consent/extra",
  "/consents",

  "/legal/terms",
  "/legal/privacy",
  "/LEGAL/unknown",
  "/legal/privacy/",
  "/legal",
  "/legal/",
  "/legal//",
  "/legals/terms",
  "/legal/terms/extra",
  "/service-info",
  "/service-info/",
  "/SERVICE-INFO/",
  "/service-infos",
  "/service-info/extra",
  "/settings/legal",
  "/settings/legal/",
  "/SETTINGS/LEGAL",
  "/invite/tok",
  "/invite/tok/",
  "/INVITE/abc-DEF",
  "/invite",
  "/invite/",
  "/invite//",
  "/invites/tok",
  "/invite/tok/extra",
  "/invite/a%2Fb",
  "/setup",
  "/setup/",
  "/SETUP",
  "/setup/extra",
  "/setups",
  "/w/acme",
  "/w/acme/a/123/view",
  "/w/acme/a/123/view/",
  "/w/acme/A/123/VIEW",
  "/w/acme/a/123",
  "/w/acme/a/123/view/extra",
  "/s/tok/attachments/123/view",
  "/s/tok/attachments/123/view/",
  "/S/tok/attachments/123/View",
  "/s/tok/attachments/123",
  "/s/tok/attachments/123/view/extra",
  "/s/tok",
  "/settings/admin",
  "/settings/audit",
  "/settings/legal",
  "/legal/privacy",
  "/w//a/123/view",
  "/s//attachments/123/view",
  "/w/acme/a//view",
  "/s/tok/attachments//view",
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
  "/w/acme/projects", "/w/acme/PROJECTS/", "/w/acme/search", "/w/acme/settings",
  "/w/acme/my-tasks", "/w/acme/notifications", "/w/acme/trash", "/w/acme/a",
  "/w/acme/OPS-DEV", "/w/acme/OPS-DEV-1", "/w/acme/PRJ-01", "/w/acme/PRJ-0",
  "/w/acme/GNT/table", "/w/acme/GNT/board", "/w/acme/GNT/calendar/",
  "/w/acme/GNT/settings/fields", "/w/acme/GNT/settings/workflow",
  "/w/acme/%47NT", "/w/acme/%47NT-1",
];

test("the boot module sends the Gantt path alongside connected project flows, to the Vue app", () => {
  assert.equal(isVueAppPath("/w/acme/GNT/gantt"), true);
  assert.equal(isVueAppPath("/w/acme/GNT/gantt/"), true);
  // React Router matched the Gantt route in any case; the boundary does too.
  assert.equal(isVueAppPath("/w/acme/GNT/Gantt"), true);
  assert.equal(isVueAppPath("/W/acme/GNT/GANTT"), true);
  assert.equal(isVueAppPath("/w/acme/GNT"), true);
  assert.equal(isVueAppPath("/w/acme/GNT/gantt/extra"), false);
  assert.equal(isVueAppPath("/w/acme/GNT/tasks"), true);
  assert.equal(isVueAppPath("/"), true);
});

test("the boot module sends valid wiki and project items, to the Vue app", () => {
  assert.equal(isVueAppPath("/w/acme/WIKI-1"), true);
  assert.equal(isVueAppPath("/w/acme/wiki-12/"), true);
  assert.equal(isVueAppPath("/w/acme/WIKI-123456789"), true);
  // parseRef refuses these; the React app handles them as before.
  assert.equal(isVueAppPath("/w/acme/WIKI-0"), false);
  assert.equal(isVueAppPath("/w/acme/WIKI-01"), false);
  assert.equal(isVueAppPath("/w/acme/WIKI-1234567890"), false);
  // Task and project-document refs now render their Vue replacement.
  assert.equal(isVueAppPath("/w/acme/PRJ-1"), true);
  assert.equal(isVueAppPath("/w/acme/GNT-1"), true);
  assert.equal(isVueAppPath("/w/acme/XWIKI-1"), true);
  assert.equal(isVueAppPath("/w/acme/wiki"), false);
});

test("the boot module sends /login, and only that path, to the Vue app", () => {
  assert.equal(isVueAppPath("/login"), true);
  assert.equal(isVueAppPath("/login/"), true);
  assert.equal(isVueAppPath("/LOGIN"), true);
  assert.equal(isVueAppPath("/login/extra"), false);
  assert.equal(isVueAppPath("/logins"), false);
});

test("the boot module sends exactly single-token invite paths to Vue", () => {
  assert.equal(isVueAppPath("/invite/tok"), true);
  assert.equal(isVueAppPath("/invite/tok/"), true);
  assert.equal(isVueAppPath("/INVITE/tok"), true);
  assert.equal(isVueAppPath("/invite/a%2Fb"), true);
  assert.equal(isVueAppPath("/invite"), false);
  assert.equal(isVueAppPath("/invite/"), false);
  assert.equal(isVueAppPath("/invite//"), false);
  assert.equal(isVueAppPath("/invite/tok/extra"), false);
  assert.equal(isVueAppPath("/invites/tok"), false);
});

test("the boot module sends /setup, and only that path, to the Vue app", () => {
  assert.equal(isVueAppPath("/setup"), true);
  assert.equal(isVueAppPath("/setup/"), true);
  assert.equal(isVueAppPath("/SETUP"), true);
  assert.equal(isVueAppPath("/setup/extra"), false);
  assert.equal(isVueAppPath("/setups"), false);
});

test("single-segment resource routes agree with the shared ref grammar", () => {
  for (const path of SAMPLES) {
    // React Router matches /w/:slug in any case, as the boundary does.
    const ref = /^\/w\/[^/]+\/([^/]+)\/?$/i.exec(path)?.[1];
    const resource = ref ? parseRef(ref) : null;
    const projectView = /\/(gantt|tasks|table|board|calendar)\/?$/i.test(path);
    const login = /^\/login\/?$/i.test(path);
    const homeOrPublic = path === "/" || /^\/legal\/[^/]+\/?$/i.test(path) || /^\/service-info\/?$/i.test(path);
    const invite = /^\/invite\/[^/]+\/?$/i.test(path);
    const setup = /^\/setup\/?$/i.test(path);
    const auth = /^\/(reset-password|magic-link|confirm-email|cancel-withdraw|consent)\/?$/i.test(path);
    const attachment = /^\/w\/[^/]+\/a\/[^/]+\/view\/?$/i.test(path) ||
      /^\/s\/[^/]+\/attachments\/[^/]+\/view\/?$/i.test(path);
    if (!projectView && !login && !homeOrPublic && !invite && !setup && !auth && !attachment) assert.equal(isVueAppPath(path), resource !== null, path);
  }
});

test("home and public pages enter Vue while admin policies remain React", () => {
  for (const path of ["/", "/legal/terms", "/legal/privacy/", "/LEGAL/unknown", "/service-info", "/SERVICE-INFO/"]) {
    assert.equal(isVueAppPath(path), true, path);
  }
  for (const path of ["/legal", "/legal/terms/extra", "/service-infos", "/service-info/extra", "/settings/legal"]) {
    assert.equal(isVueAppPath(path), false, path);
  }
});

test("attachment viewers boot Vue while public share and admin pages retain React", () => {
  for (const path of ["/w/acme/a/123/view", "/w/acme/a/123/view/", "/W/acme/A/123/VIEW",
    "/s/tok/attachments/123/view", "/s/tok/attachments/123/view/", "/S/tok/attachments/123/View"]) {
    assert.equal(isVueAppPath(path), true, path);
  }
  for (const path of ["/s/tok", "/settings/admin", "/settings/audit", "/settings/legal",
    "/w/acme/a/123", "/w/acme/a/123/view/extra", "/s/tok/attachments/123", "/s/tok/attachments/123/view/extra"]) {
    assert.equal(isVueAppPath(path), false, path);
  }
});

test("public policy pages remain Vue after the viewer merge", () => {
  for (const path of ["/legal/privacy", "/LEGAL/terms/"]) {
    assert.equal(isVueAppPath(path), true, path);
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
