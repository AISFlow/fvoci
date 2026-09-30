import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

test("the Vue my-tasks page uses the shared infinite query and does not import React", () => {
  const page = source("../../pages/MyTasksPage.vue");
  assert.match(page, /myTasksQuery\(workspaceId\.value\)/);
  assert.match(page, /mergeTaskListPages/);
  assert.match(page, /groupTasksByProject/);
  assert.match(page, /enabled: Boolean\(workspaceId\.value\)/);
  assert.match(page, /status === 400/);
  assert.match(page, /tasks\.refetch\(\)/);
  assert.match(page, /tasks\.fetchNextPage\(\)/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
  assert.doesNotMatch(page, /TaskListLoadMore/);

  const row = source("MyTaskRow.vue");
  assert.match(row, /itemPath/);
  assert.match(row, /isVueAppPath/);
  assert.match(row, /RouterLink/);
  assert.match(row, /:href="href"/);
  assert.doesNotMatch(row, /from ["']react["']/);
  assert.doesNotMatch(row, /from ["']@tanstack\/react-query["']/);
});
