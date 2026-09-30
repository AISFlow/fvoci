import { queryOptions } from "@/lib/query-options";
import { api, ensureOk } from "@/lib/api";
import type { components } from "@/generated/api";

export type TreeNode = components["schemas"]["TreeNodeResponse"];
export type DocumentMeta = components["schemas"]["DocumentMetaResponse"];
export type DocumentBody = components["schemas"]["BodyResponse"];

export function treeQuery(workspaceId: string, tag?: string) {
  return queryOptions({
    queryKey: tag ? ["tree", workspaceId, tag] as const : ["tree", workspaceId] as const,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tree", {
          signal,
          params: { path: { workspace_id: workspaceId }, query: tag ? { tag } : {} },
        }),
      ),
    enabled: Boolean(workspaceId),
  });
}

/** Authorized workspace wiki/project discovery; the default tree remains wiki-only. */
export function wikiDiscoveryQuery(workspaceId: string, tag?: string) {
  return queryOptions({
    queryKey: ["wiki-discovery", workspaceId, tag ?? ""] as const,
    queryFn: async ({ signal }) => ensureOk(await api.GET("/api/v1/workspaces/{workspace_id}/wiki-discovery", {
      signal,
      params: { path: { workspace_id: workspaceId }, query: tag ? { tag } : {} },
    })),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export function documentMetaQuery(workspaceId: string, documentId: string) {
  return queryOptions({
    queryKey: ["document", workspaceId, documentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(documentId),
  });
}

export function projectDocumentMetaQuery(
  workspaceId: string,
  projectId: string,
  documentId: string,
) {
  return queryOptions({
    queryKey: ["project-document", workspaceId, projectId, documentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
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
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId) && Boolean(documentId),
  });
}

export function documentBodyQuery(workspaceId: string, documentId: string) {
  return queryOptions({
    queryKey: ["document-body", workspaceId, documentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(documentId),
  });
}

export function trashQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["trash", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/trash", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
  });
}

export function ancestorsQuery(workspaceId: string, documentId: string) {
  return queryOptions({
    queryKey: ["ancestors", workspaceId, documentId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/ancestors", {
          params: {
            path: { workspace_id: workspaceId, document_id: documentId },
          },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(documentId),
  });
}
