import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import {
  invalidateTaskCaches,
  invalidateTaskStreamResyncCaches,
} from "@/features/tasks/task-cache";
import { subscribeTaskStream } from "@/lib/task-stream";

/**
 * Subscribe to project task invalidation for the current page context.
 * Gantt collection route (#130): call `useTaskStream(workspaceId, projectId)` from the
 * future Gantt page the same way as `ProjectCollectionPage`.
 */
export function useTaskStream(workspaceId: string | undefined, projectId: string | undefined) {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!workspaceId || !projectId) return;
    const sub = subscribeTaskStream(workspaceId, projectId, {
      onResync: () => {
        invalidateTaskStreamResyncCaches(queryClient, workspaceId, projectId);
      },
      onTask: (hint) => {
        void invalidateTaskCaches(queryClient, workspaceId, projectId, hint.taskId);
      },
    });
    return () => sub.close();
  }, [workspaceId, projectId, queryClient]);
}
