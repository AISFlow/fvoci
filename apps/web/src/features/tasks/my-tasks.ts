// Source features/tasks/my-tasks-view.tsx grouping and routes/w.$slug.my-tasks.tsx query.

/** Source `OPEN_ASSIGNED_QUERY`: open tasks assigned to the viewer, due soonest first. */
export const OPEN_ASSIGNED_QUERY = JSON.stringify({
  filters: { assigneeId: "me", openOnly: true },
  sort: [{ field: "due", direction: "asc" }],
});

/** Groups in first-seen order so the server's due-date order holds across projects. */
export function groupTasksByProject<T extends { projectId: string }>(
  items: readonly T[],
): Array<[string, T[]]> {
  const grouped = new Map<string, T[]>();
  for (const item of items) {
    const bucket = grouped.get(item.projectId) ?? [];
    bucket.push(item);
    grouped.set(item.projectId, bucket);
  }
  return [...grouped.entries()];
}
