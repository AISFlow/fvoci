import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { onScopeDispose, toValue, watch, type MaybeRefOrGetter } from "vue";
import {
  invalidateTaskCaches,
  invalidateTaskStreamResyncCaches,
} from "@/features/tasks/task-cache";
import { ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { subscribeTaskStream } from "@/lib/task-stream";
/** Existing authorized project streams feed mounted aggregate consumers too. */
export function useTaskStreams(
  workspaceId: MaybeRefOrGetter<string | undefined>,
  projectIds: MaybeRefOrGetter<readonly string[]>,
): void {
  const queryClient = useQueryClient();
  const me = useQuery(meQuery);
  let generation = 0;
  watch(
    () =>
      [
        toValue(workspaceId),
        [...new Set(toValue(projectIds))].sort().join(","),
        me.data.value?.userId,
        me.data.value?.sessionId,
        me.error.value instanceof ProblemError && me.error.value.status === 401,
      ] as const,
    ([ws, ids, actor, credential, retired], _previous, onCleanup) => {
      const lifetime = ++generation;
      if (!ws || !actor || !ids || retired) return;
      const current = () =>
        lifetime === generation &&
        queryClient.getQueryData<{ userId: string; sessionId: string }>(["auth", "me"])?.userId ===
          actor &&
        queryClient.getQueryData<{ sessionId: string }>(["auth", "me"])?.sessionId === credential;
      const subscriptions = ids.split(",").map((project) =>
        subscribeTaskStream(ws, project, {
          onResync: () => {
            if (current()) invalidateTaskStreamResyncCaches(queryClient, ws, project);
          },
          onTask: (hint) => {
            if (current())
              invalidateTaskCaches(queryClient, ws, project, hint.taskId).catch(reportError);
          },
        }),
      );
      onCleanup(() => {
        generation++;
        subscriptions.forEach((subscription) => {
          subscription.close();
        });
      });
    },
    { immediate: true },
  );
  onScopeDispose(() => {
    generation++;
  });
}
export function useTaskStream(
  workspaceId: MaybeRefOrGetter<string | undefined>,
  projectId: MaybeRefOrGetter<string | undefined>,
): void {
  useTaskStreams(workspaceId, () => {
    const id = toValue(projectId);
    return id ? [id] : [];
  });
}
function reportError(error: unknown): void {
  console.error("task stream cache refresh failed", error);
}
