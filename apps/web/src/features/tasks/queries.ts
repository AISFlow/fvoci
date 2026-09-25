import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

export type TaskMeta = components["schemas"]["TaskMetaOutput"];
export type TaskListItem = components["schemas"]["TaskListItemOutput"];
export type TaskDetail = components["schemas"]["TaskOutput"];
export type CreateTaskBody = components["schemas"]["CreateTaskBody"];
export type TaskListResponse = components["schemas"]["TaskListResponse"];
export type LabelItem = components["schemas"]["LabelOutput"];
export type LabelListResponse = components["schemas"]["LabelListResponse"];

export function taskListQuery(workspaceId: string, projectId: string) {
  return infiniteQueryOptions({
    queryKey: ["tasks", workspaceId, projectId] as const,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
          params: {
            path: { workspace_id: workspaceId, project_id: projectId },
            query: pageParam ? { cursor: pageParam } : undefined,
          },
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
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
    retry: false,
  });
}

export function projectLabelsQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["labels", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export function projectLabelsQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["labels", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}
