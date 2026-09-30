import { api, ensureOk, ProblemError } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";

// Task origin requests shared by the React and Vue origin panels: the tasks a
// document started, or the documents a task came from, under the current
// permission (not the one the link was created with).

export function originErrorText(error: unknown, fallback: string): string {
  return error instanceof ProblemError ? error.title : fallback;
}

export function taskOriginsQuery(
  workspaceId: string,
  target: { documentId?: string; taskId?: string },
  after: string | null,
) {
  const { documentId, taskId } = target;
  return queryOptions({
    queryKey: ["task-origins", workspaceId, documentId ?? taskId, after] as const,
    enabled: Boolean(workspaceId && (documentId || taskId)),
    retry: false,
    queryFn: async () =>
      documentId
        ? ensureOk(
            await api.GET(
              "/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-origins",
              {
                params: {
                  path: { workspace_id: workspaceId, document_id: documentId },
                  query: { after: after ?? undefined, limit: 50 },
                },
              },
            ),
          )
        : ensureOk(
            await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/origin", {
              params: {
                path: { workspace_id: workspaceId, task_id: taskId! },
                query: { after: after ?? undefined, limit: 50 },
              },
            }),
          ),
  });
}

export function documentTaskProjectsQuery(workspaceId: string, documentId: string | undefined) {
  return queryOptions({
    queryKey: ["task-projects", workspaceId, documentId] as const,
    enabled: Boolean(documentId),
    retry: false,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-projects", {
          params: { path: { workspace_id: workspaceId, document_id: documentId! } },
        }),
      ),
  });
}

export async function createTaskFromDocument(
  workspaceId: string,
  documentId: string,
  body: { projectId: string; requestId: string; title: string },
) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/tasks", {
      params: { path: { workspace_id: workspaceId, document_id: documentId } },
      body: { projectId: body.projectId, requestId: body.requestId, task: { title: body.title } },
    }),
  );
}

export async function createOriginProject(workspaceId: string, key: string, name: string) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/projects", {
      params: { path: { workspace_id: workspaceId } },
      body: { key: key.trim().toUpperCase(), name: name.trim(), visibility: "workspace" },
    }),
  );
}
