import type { components } from "@/generated/api";

type GeneratedTaskList = components["schemas"]["TaskListResponse"];

/**
 * Compile latch: assignment is `false` until generated list JSON requires `nextCursor` and
 * `statusCounts`. Do not invent those fields or count one page as the catalog.
 */
export const GENERATED_TASK_LIST_PAGINATION: GeneratedTaskList extends {
  nextCursor: string | null;
  statusCounts: readonly { statusId: string; count: number }[];
}
  ? true
  : false = false;

export type TaskListStatusCount = {
  statusId: string;
  count: number;
};

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

export function taskListHasMore(page: Pick<TaskListPage<unknown>, "nextCursor">): boolean {
  return page.nextCursor !== null;
}

export function statusCountFor(
  statusCounts: readonly TaskListStatusCount[],
  statusId: string,
): number | undefined {
  return statusCounts.find((row) => row.statusId === statusId)?.count;
}
