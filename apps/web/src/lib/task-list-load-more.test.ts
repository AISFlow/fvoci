import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { t } from "@fvoci/i18n";
import { TaskListLoadMore } from "../features/tasks/task-list-load-more.ts";

test("rejected second page shows a Korean alert and keeps load-more usable", () => {
  const html = renderToStaticMarkup(
    createElement(TaskListLoadMore, {
      error: t("task.list.loadMoreFailed"),
      pending: false,
      onLoadMore: () => {
        throw new Error("should remain clickable in markup");
      },
    }),
  );
  assert.match(html, /role="alert"/);
  assert.match(html, /태스크를 더 불러오지 못했습니다\. 다시 시도해 주세요\./);
  assert.match(html, /더 보기/);
  assert.doesNotMatch(html, /\sdisabled(=|>|\s)/);
});
