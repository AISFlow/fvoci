import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

export type VersionedBody = components["schemas"]["VersionedBodyResponse"];
export type BodySaveCommand = components["schemas"]["SaveVersionedBodyInput"];
export type BodySaveResult = components["schemas"]["SaveVersionedBodyResponse"];

export async function readVersionedBody(
  workspaceId: string,
  documentId: string,
  signal?: AbortSignal,
  projectId?: string | null,
  kind: "document" | "task" = "document",
) {
  if (kind === "task")
    return ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/body/versioned", {
        signal,
        params: { path: { workspace_id: workspaceId, task_id: documentId } },
      }),
    );
  if (projectId)
    return ensureOk(
      await api.GET(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body/versioned",
        {
          signal,
          params: {
            path: { workspace_id: workspaceId, project_id: projectId, document_id: documentId },
          },
        },
      ),
    );
  return ensureOk(
    await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body/versioned", {
      signal,
      params: { path: { workspace_id: workspaceId, document_id: documentId } },
    }),
  );
}

export async function saveVersionedBody(
  workspaceId: string,
  documentId: string,
  body: BodySaveCommand,
  projectId?: string | null,
  kind: "document" | "task" = "document",
) {
  if (kind === "task")
    return ensureOk(
      await api.PUT("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/body/versioned", {
        body,
        params: { path: { workspace_id: workspaceId, task_id: documentId } },
      }),
    );
  if (projectId)
    return ensureOk(
      await api.PUT(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body/versioned",
        {
          body,
          params: {
            path: { workspace_id: workspaceId, project_id: projectId, document_id: documentId },
          },
        },
      ),
    );
  return ensureOk(
    await api.PUT("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body/versioned", {
      params: { path: { workspace_id: workspaceId, document_id: documentId } },
      body,
    }),
  );
}
