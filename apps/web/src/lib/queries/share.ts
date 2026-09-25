import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";

export type StarItem = components["schemas"]["StarItemOutput"];
export type RecentItem = components["schemas"]["RecentItemOutput"];
export type ShareLink = components["schemas"]["ShareLinkOutput"];
export type ShareTreeNode = components["schemas"]["TreeNodeResponse"];

export function starsQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["stars", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/stars", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export function recentQuery(workspaceId: string, limit = 8) {
  return queryOptions({
    queryKey: ["recent", workspaceId, limit] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/recent", {
          params: { path: { workspace_id: workspaceId }, query: { limit } },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export type ShareDocumentTarget = { documentId: string; projectId: string | null };

export function documentShareLinksQuery(workspaceId: string, target: ShareDocumentTarget) {
  return queryOptions({
    queryKey: ["share-links", workspaceId, target.documentId] as const,
    queryFn: async () =>
      ensureOk(
        target.projectId === null
          ? await api.GET("/api/v1/workspaces/{workspace_id}/documents/{id}/share-links", {
              params: { path: { workspace_id: workspaceId, id: target.documentId } },
            })
          : await api.GET(
              "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{id}/share-links",
              {
                params: {
                  path: {
                    workspace_id: workspaceId,
                    project_id: target.projectId,
                    id: target.documentId,
                  },
                },
              },
            ),
      ),
    retry: false,
  });
}

const noRetryOn404 = (failureCount: number, error: unknown) =>
  !(error instanceof ProblemError && (error.status === 404 || error.status === 429)) &&
  failureCount < 2;

export function sharePublicMetaQuery(token: string) {
  return queryOptions({
    queryKey: ["share-public", token] as const,
    queryFn: async () =>
      ensureOk(await api.GET("/api/v1/share/{token}", { params: { path: { token } } })),
    retry: noRetryOn404,
  });
}

export function sharePublicTreeQuery(token: string, enabled: boolean) {
  return queryOptions({
    queryKey: ["share-tree", token] as const,
    queryFn: async () =>
      ensureOk(await api.GET("/api/v1/share/{token}/tree", { params: { path: { token } } })),
    enabled,
    retry: noRetryOn404,
  });
}

/** `format=fragment`: server-sanitized body HTML only (no document shell). */
export function sharePublicBodyQuery(token: string, documentId: string | null, enabled: boolean) {
  return queryOptions({
    queryKey: ["share-body", token, documentId] as const,
    queryFn: async (): Promise<string> => {
      const result = documentId
        ? await api.GET("/api/v1/share/{token}/documents/{document_id}", {
            params: { path: { token, document_id: documentId }, query: { format: "fragment" } },
            parseAs: "text",
          })
        : await api.GET("/api/v1/share/{token}/body", {
            params: { path: { token }, query: { format: "fragment" } },
            parseAs: "text",
          });
      if (!result.response.ok) {
        const code = (result.error as { code?: string } | undefined)?.code;
        throw new ProblemError(result.response.status, code);
      }
      const contentType = result.response.headers.get("content-type") ?? "";
      if (!contentType.includes("text/html")) {
        throw new ProblemError(500);
      }
      return typeof result.data === "string" ? result.data : "";
    },
    enabled,
    retry: noRetryOn404,
  });
}
