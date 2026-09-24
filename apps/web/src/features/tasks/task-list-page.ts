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

export type NamedStatus = {
  id: string;
  name: string;
};

export type TaskStatusSection<T> = {
  id: string;
  name: string;
  items: T[];
  count: number;
  known: boolean;
};

/** Keep loaded rows visible even when the server count is 0 or the status is unknown. */
export function visibleTaskStatusSections<T extends { statusId: string }>(
  items: readonly T[],
  statuses: readonly NamedStatus[],
  statusCounts: readonly TaskListStatusCount[],
  otherName: string,
): TaskStatusSection<T>[] {
  const byStatus = new Map<string, T[]>();
  for (const status of statuses) byStatus.set(status.id, []);
  const other: T[] = [];
  for (const item of items) {
    const group = byStatus.get(item.statusId);
    if (group) group.push(item);
    else other.push(item);
  }
  const sections: TaskStatusSection<T>[] = [];
  for (const status of statuses) {
    const group = byStatus.get(status.id) ?? [];
    const count = statusCountFor(statusCounts, status.id) ?? 0;
    if (count === 0 && group.length === 0) continue;
    sections.push({ id: status.id, name: status.name, items: group, count, known: true });
  }
  if (other.length > 0) {
    sections.push({
      id: "__other",
      name: otherName,
      items: other,
      count: other.length,
      known: false,
    });
  }
  return sections;
}
