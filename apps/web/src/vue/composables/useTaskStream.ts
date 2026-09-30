import { useQueryClient } from "@tanstack/vue-query";
import { toValue, watch, type MaybeRefOrGetter } from "vue";
import {
  invalidateTaskCaches,
  invalidateTaskStreamResyncCaches,
} from "@/features/tasks/task-cache";
import { subscribeTaskStream } from "@/lib/task-stream";

/**
 * The project's task invalidation stream (lib/task-stream.ts, shared with the
 * React app's hooks/use-task-stream.ts): a task hint or a resync invalidates
 * the same query keys.
 */
export function useTaskStream(
  workspaceId: MaybeRefOrGetter<string | undefined>,
  projectId: MaybeRefOrGetter<string | undefined>,
): void {
  const queryClient = useQueryClient();
  watch(
    () => [toValue(workspaceId), toValue(projectId)] as const,
    ([ws, project], _previous, onCleanup) => {
      if (!ws || !project) return;
      const subscription = subscribeTaskStream(ws, project, {
        onResync: () => invalidateTaskStreamResyncCaches(queryClient, ws, project),
        onTask: (hint) => void invalidateTaskCaches(queryClient, ws, project, hint.taskId),
      });
      onCleanup(() => subscription.close());
    },
    { immediate: true },
  );
}
