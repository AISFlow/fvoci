import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { GENERATED_TASK_LIST_PAGINATION } from "./task-list-page";

export type TaskMeta = components["schemas"]["TaskMetaOutput"];
export type TaskDetail = components["schemas"]["TaskOutput"];
export type CreateTaskBody = components["schemas"]["CreateTaskBody"];
export type TaskListResponse = components["schemas"]["TaskListResponse"];

export function taskListQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["tasks", workspaceId, projectId] as const,
    queryFn: async () => {
      if (GENERATED_TASK_LIST_PAGINATION) {
        throw new Error("wire_generated_list_cursor");
      }
      return ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      );
    },
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export function taskQuery(workspaceId: string, taskId: string) {
  return queryOptions({
    queryKey: ["task", workspaceId, taskId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(taskId),
    retry: false,
  });
}

export function stringIds(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

export function formatEstimate(value: unknown): string | null {
  if (value == null || value === "") return null;
  return String(value);
}
