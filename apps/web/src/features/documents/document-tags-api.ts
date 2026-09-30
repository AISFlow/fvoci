import type { QueryClient } from "@tanstack/query-core";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { documentTagPoolQuery, type DocumentTag } from "@/lib/queries/collections";

// Document tag requests shared by the React and Vue tag bars. A wiki document
// when `projectId` is null, a project document otherwise.

export async function assignDocumentTag(
  workspaceId: string,
  documentId: string,
  projectId: string | null,
  tagId: string,
): Promise<DocumentTag> {
  const result = projectId
    ? await api.POST(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags",
        {
          params: {
            path: { workspace_id: workspaceId, project_id: projectId, document_id: documentId },
          },
          body: { tagId },
        },
      )
    : await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags", {
        params: { path: { workspace_id: workspaceId, document_id: documentId } },
        body: { tagId },
      });
  return ensureOk(result);
}

/** Creates a workspace tag; a concurrent create of the same name reuses the existing tag (source). */
export async function createDocumentTag(
  queryClient: QueryClient,
  workspaceId: string,
  name: string,
) {
  try {
    return await ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/document-tags", {
        params: { path: { workspace_id: workspaceId } },
        body: { name, color: "gray" },
      }),
    );
  } catch (err) {
    if (err instanceof ProblemError && err.status === 409) {
      const page = await queryClient.fetchQuery({
        ...documentTagPoolQuery(workspaceId, name),
        staleTime: 0,
      });
      const existing = page.items.find((tag) => tag.name.toLowerCase() === name.toLowerCase());
      if (existing) return existing;
    }
    throw err;
  }
}

export async function removeDocumentTag(
  workspaceId: string,
  documentId: string,
  projectId: string | null,
  tagId: string,
) {
  const result = projectId
    ? await api.DELETE(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags/{tag_id}",
        {
          params: {
            path: {
              workspace_id: workspaceId,
              project_id: projectId,
              document_id: documentId,
              tag_id: tagId,
            },
          },
        },
      )
    : await api.DELETE(
        "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags/{tag_id}",
        {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId, tag_id: tagId },
          },
        },
      );
  return ensureOk(result);
}
