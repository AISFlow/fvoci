import { queryOptions } from "@tanstack/react-query";
import { api, ensureOk } from "@/lib/api";
import type { components } from "@/generated/api";

export type TreeNode = components["schemas"]["TreeNodeResponse"];
export type DocumentMeta = components["schemas"]["DocumentMetaResponse"];
export type DocumentBody = components["schemas"]["BodyResponse"];

export function treeQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["tree", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tree", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
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
