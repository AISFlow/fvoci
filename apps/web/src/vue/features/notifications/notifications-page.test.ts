import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue notifications page PATCHes read/archive, POSTs read-all, and follows hrefs", () => {
  const page = source("../../pages/NotificationsPage.vue");
  assert.match(page, /notificationListQuery/);
  assert.match(page, /enabled: Boolean\(id\)/);
  assert.match(
    page,
    /api\.PATCH\("\/api\/v1\/workspaces\/\{workspace_id\}\/notifications\/\{id\}"/,
  );
  assert.match(page, /body: \{ read: true \}/);
  assert.match(page, /body: \{ archived: !item\.archivedAt \}/);
  assert.match(
    page,
    /api\.POST\("\/api\/v1\/workspaces\/\{workspace_id\}\/notifications\/read-all"/,
  );
  assert.match(page, /notificationHref/);
  assert.match(page, /followAppHref\(href, router\)/);
  assert.match(page, /query: \{ filter, cursor: pageParam \}/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
});

await test("followAppHref stays in Vue only for live app-boundary paths", () => {
  const navigation = source("../../session/navigation.ts");
  assert.match(navigation, /isVueAppPath\(href\.split/);
  assert.match(navigation, /router\.push\(href\)/);
  assert.match(navigation, /window\.location\.assign\(href\)/);
});
