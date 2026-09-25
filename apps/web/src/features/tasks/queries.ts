import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { parentListViewQuery } from "./task-parent-query";

export type TaskMeta = components["schemas"]["TaskMetaOutput"];
export type TaskListItem = components["schemas"]["TaskListItemOutput"];
export type TaskDetail = components["schemas"]["TaskOutput"];
export type CreateTaskBody = components["schemas"]["CreateTaskBody"];
export type TaskListResponse = components["schemas"]["TaskListResponse"];
export type LabelItem = components["schemas"]["LabelOutput"];
export type LabelListResponse = components["schemas"]["LabelListResponse"];
export type MilestoneItem = components["schemas"]["MilestoneOutput"];
export type MilestoneListResponse = components["schemas"]["MilestoneListResponse"];
export type TaskDependency = components["schemas"]["TaskDependencyOutput"];

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

export function taskParentListQuery(
  workspaceId: string,
  projectId: string,
  childType: string,
  excludeTaskId: string,
  title?: string,
) {
  const query = parentListViewQuery(childType, title);
  return infiniteQueryOptions({
    queryKey: ["task-parents", workspaceId, projectId, childType, excludeTaskId, title ?? ""] as const,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
          params: {
            path: { workspace_id: workspaceId, project_id: projectId },
            query: {
              query,
              limit: 20,
              ...(pageParam ? { cursor: pageParam } : {}),
            },
          },
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
    enabled: Boolean(workspaceId) && Boolean(projectId) && childType !== "epic",
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

export type TaskActivityFilter = "all" | "comments" | "changes";

export function taskActivityQuery(
  workspaceId: string,
  taskId: string,
  filter: TaskActivityFilter,
) {
  return infiniteQueryOptions({
    queryKey: ["task-activity", workspaceId, taskId, filter] as const,
    queryFn: async ({ pageParam }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity", {
          params: {
            path: { workspace_id: workspaceId, task_id: taskId },
            query: {
              filter,
              limit: 50,
              ...(pageParam ? { cursor: pageParam } : {}),
            },
          },
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
    enabled: Boolean(workspaceId) && Boolean(taskId),
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

export function projectMilestonesQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["milestones", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}
