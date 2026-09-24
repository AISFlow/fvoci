import type { components } from "@/generated/api";

export type TaskListStatusCount = components["schemas"]["TaskStatusCountOutput"];

/** Source `taskListOutput` page merge shape. */
export type TaskListPage<TItem> = {
  items: TItem[];
  nextCursor: string | null;
  statusCounts: TaskListStatusCount[];
  truncated?: boolean;
};

/** Source `appendTaskListPage`: concatenate items, keep first-page statusCounts, take page cursor. */
export function appendTaskListPage<TItem>(
  previous: TaskListPage<TItem> | undefined,
  page: TaskListPage<TItem>,
): TaskListPage<TItem> {
  if (previous === undefined) return page;
  return {
    items: [...previous.items, ...page.items],
    nextCursor: page.nextCursor,
    statusCounts: previous.statusCounts,
    ...(page.truncated !== undefined ? { truncated: page.truncated } : {}),
  };
}

export function mergeTaskListPages<TItem>(
  pages: readonly TaskListPage<TItem>[],
): TaskListPage<TItem> | undefined {
  let merged: TaskListPage<TItem> | undefined;
  for (const page of pages) merged = appendTaskListPage(merged, page);
  return merged;
}

export function taskListHasMore(page: Pick<TaskListPage<unknown>, "nextCursor">): boolean {
  return page.nextCursor !== null;
}

export function statusCountFor(
  statusCounts: readonly TaskListStatusCount[],
  statusId: string,
): number | undefined {
  return statusCounts.find((row) => row.statusId === statusId)?.count;
}
