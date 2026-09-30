import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

// Document page requests shared by the React and Vue document pages: a wiki
// document when `projectId` is null, a project document (project permission)
// otherwise.

export type PatchDocumentBody = components["schemas"]["PatchDocumentBody"];

export interface DocumentScope {
  workspaceId: string;
  documentId: string;
  projectId: string | null;
}

export async function patchDocument(scope: DocumentScope, body: PatchDocumentBody) {
  const { workspaceId, documentId, projectId } = scope;
  return projectId
    ? ensureOk(
        await api.PATCH(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                project_id: projectId,
                document_id: documentId,
              },
            },
            body,
          },
        ),
      )
    : ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/documents/{document_id}", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
          body,
        }),
      );
}

export async function trashDocument(scope: DocumentScope) {
  const { workspaceId, documentId, projectId } = scope;
  return projectId
    ? ensureOk(
        await api.POST(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/trash",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                project_id: projectId,
                document_id: documentId,
              },
            },
          },
        ),
      )
    : ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/trash", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
        }),
      );
}

export async function moveDocument(scope: DocumentScope, newParentId: string) {
  const { workspaceId, documentId, projectId } = scope;
  return projectId
    ? ensureOk(
        await api.POST(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/move",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                project_id: projectId,
                document_id: documentId,
              },
            },
            body: { newParentId },
          },
        ),
      )
    : ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/move", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
          body: { newParentId },
        }),
      );
}
