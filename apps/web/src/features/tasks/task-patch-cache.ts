import type { QueryClient } from "@tanstack/react-query";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import type { TaskDetail, TaskMeta } from "./queries";

/**
 * Fold a PATCH `TaskMetaOutput` into the cached `TaskOutput`. Detail-only fields
 * stay; the parent preview is kept only while it still names `meta.parentId`.
 */
export function mergeTaskMeta(
  cached: TaskDetail | undefined,
  meta: TaskMeta,
): TaskDetail | undefined {
  if (!cached || cached.id !== meta.id) return cached;
  return {
    ...cached,
    ...meta,
    parent: cached.parent?.id === meta.parentId ? cached.parent : null,
  };
}

/**
 * PATCH success: the response is the committed row, so the detail cache takes it
 * before the mutation settles. Awaiting the refetch alone is not enough — when
 * stream hints restart the GET twice, TanStack settles the first awaiter with the
 * cancelled middle fetch while the cache still holds the pre-PATCH row, and a
 * form that compares drafts with that row loses the next edit. An in-flight GET
 * may have read the row before the commit, so it is cancelled first.
 */
export async function settleTaskPatch(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
  meta: TaskMeta,
): Promise<void> {
  const taskKey = ["task", workspaceId, meta.id] as const;
  await queryClient.cancelQueries({ queryKey: taskKey, exact: true });
  queryClient.setQueryData<TaskDetail>(taskKey, (cached) => mergeTaskMeta(cached, meta));
  await invalidateTaskCaches(queryClient, workspaceId, projectId, meta.id);
}
