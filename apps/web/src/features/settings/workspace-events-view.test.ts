import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { t } from "@fvoci/i18n";
import { WorkspaceEventsView, type WorkspaceEvent } from "./workspace-events-view.ts";

function event(id: string, verb: string, createdAt: string): WorkspaceEvent {
  return {
    id,
    verb,
    workspaceId: "01900000-0000-7000-8000-000000000001",
    actorUserId: null,
    targetType: null,
    targetId: null,
    payload: {},
    channel: "web",
    createdAt,
  };
}

function render(overrides: Partial<Parameters<typeof WorkspaceEventsView>[0]> = {}) {
  return renderToStaticMarkup(
    createElement(WorkspaceEventsView, {
      items: [],
      timeZone: "Asia/Seoul",
      loading: false,
      error: null,
      onRetry: () => {},
      hasMore: false,
      loadingMore: false,
      loadMoreError: null,
      onLoadMore: () => {},
      ...overrides,
    }),
  );
}

test("event rows show verb and Seoul-local time under the Korean activity title", () => {
  const html = render({
    items: [
      event("a", "workspace.created", "2026-09-27T15:30:00.000Z"),
      event("b", "project.updated", "2026-09-28T01:05:00.000Z"),
    ],
  });
  assert.match(html, /<h2[^>]*>활동<\/h2>/);
  assert.ok(
    html.indexOf("workspace.created") < html.indexOf("project.updated"),
    "server order kept",
  );
  assert.match(html, /2026\. 09\. 28\. 00:30/);
  assert.match(html, /2026\. 09\. 28\. 10:05/);
  assert.doesNotMatch(html, /활동이 없습니다/);
  assert.doesNotMatch(html, /더 보기/, "no load-more without a next cursor");
});

test("empty, loading and first-page error states", () => {
  assert.match(render(), /활동이 없습니다/);
  const loading = render({ loading: true });
  assert.match(loading, /role="status"[^>]*>불러오는 중…/);
  assert.doesNotMatch(loading, /활동이 없습니다/);
  const failed = render({ error: "불러오지 못했습니다.", hasMore: true });
  assert.match(failed, /role="alert"[^>]*>불러오지 못했습니다\./);
  assert.match(failed, /다시 시도/);
  assert.doesNotMatch(failed, /활동이 없습니다/);
  assert.doesNotMatch(failed, /더 보기/);
});

test("next cursor offers load-more; a failed later page keeps rows and stays retryable", () => {
  const items = [event("a", "task.created", "2026-09-28T00:00:00.000Z")];
  const more = render({ items, hasMore: true });
  assert.match(more, /더 보기/);
  assert.doesNotMatch(more, /\sdisabled(=|>|\s)/);
  const pending = render({ items, hasMore: true, loadingMore: true });
  assert.match(pending, /disabled/);
  const failed = render({
    items,
    hasMore: true,
    loadMoreError: t("settings.activity.loadMoreFailed"),
  });
  assert.match(failed, /task\.created/);
  assert.match(failed, /role="alert"[^>]*>활동을 더 불러오지 못했습니다\. 다시 시도해 주세요\./);
  assert.match(failed, /더 보기/);
});
