import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";

export type TimerCommand = components["schemas"]["TimerCommandBody"];

export function taskStopwatchQuery(
  actor: string,
  workspaceId: string,
  taskId: string,
  sessionId: string,
) {
  return queryOptions({
    queryKey: ["task-timer", actor, sessionId, workspaceId, taskId] as const,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
          signal,
        }),
      ),
    enabled: Boolean(actor && sessionId && workspaceId && taskId),
    retry: false,
    // Covers other tabs/new sessions; DB remains the only run owner. A server
    // hint can also refetch this exact query, never a generic entity cache.
    refetchInterval: 5000,
  });
}

export async function sendTimerCommand(workspaceId: string, taskId: string, body: TimerCommand) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer", {
      params: { path: { workspace_id: workspaceId, task_id: taskId } },
      body,
    }),
  );
}

export function ownerStopwatchQuery(actor: string, sessionId: string) {
  return queryOptions({
    queryKey: ["task-timer-owner", actor, sessionId] as const,
    queryFn: async ({ signal }) => ensureOk(await api.GET("/api/v1/me/task-timer", { signal })),
    enabled: Boolean(actor && sessionId),
    retry: false,
    refetchInterval: 5000,
  });
}
