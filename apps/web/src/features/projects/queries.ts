import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

export type ProjectListItem = components["schemas"]["ProjectListItemOutput"];
export type Project = components["schemas"]["ProjectOutput"];
export type CreateProjectBody = components["schemas"]["CreateProjectBody"];
export type ProjectMember = components["schemas"]["MemberResponse"];
export type Workflow = components["schemas"]["WorkflowOutput"];
export type WorkflowStatus = components["schemas"]["WorkflowStatusOutput"];

export function projectsQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["projects", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export function projectQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["project", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export function projectMembersQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["project-members", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export function workflowQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["workflow", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export function findProjectByKey(
  items: readonly ProjectListItem[] | undefined,
  key: string,
): ProjectListItem | undefined {
  const canonical = key.toUpperCase();
  return items?.find((project) => project.key.toUpperCase() === canonical);
}

export function backlogStatusId(statuses: readonly WorkflowStatus[]): string | null {
  const backlog = statuses.find((status) => status.category === "backlog");
  return backlog?.id ?? statuses[0]?.id ?? null;
}
