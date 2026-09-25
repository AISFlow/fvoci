import { queryOptions } from "@tanstack/react-query";
import { api, ensureOk } from "@/lib/api";

export const setupStatusQuery = queryOptions({
  queryKey: ["setup", "status"],
  queryFn: async () => ensureOk(await api.GET("/api/v1/setup")),
});

export const meQuery = queryOptions({
  queryKey: ["auth", "me"],
  queryFn: async () => ensureOk(await api.GET("/api/v1/auth/me")),
  retry: false,
});

export const workspacesQuery = queryOptions({
  queryKey: ["me", "workspaces"],
  queryFn: async () => ensureOk(await api.GET("/api/v1/me/workspaces")),
});

export function workspaceMetaQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspaces", workspaceId],
    queryFn: async () =>
      ensureOk(await api.GET("/api/v1/workspaces/{workspace_id}", {
        params: { path: { workspace_id: workspaceId } },
      })),
  });
}

export function membersQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspaces", workspaceId, "members"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/members", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });
}

export type SearchTab = "all" | "document" | "task" | "attachment" | "comment";

export function searchQuery(
  workspaceId: string,
  q: string,
  tab: SearchTab,
  projectId?: string,
  cursor?: string,
) {
  return queryOptions({
    queryKey: ["search", workspaceId, q, tab, projectId ?? "", cursor ?? ""] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/search", {
          params: {
            path: { workspace_id: workspaceId },
            query: {
              q,
              type: tab,
              ...(projectId ? { projectId } : {}),
              ...(cursor ? { cursor } : {}),
            },
          },
        }),
      ),
    enabled: Boolean(workspaceId) && q.trim().length > 0,
    retry: false,
  });
}

export function invitationPublicQuery(token: string) {
  return queryOptions({
    queryKey: ["invitation", token],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/invitations/{token}", {
          params: { path: { token } },
        }),
      ),
    retry: false,
  });
}

export function workspaceApiTokensQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspaces", workspaceId, "api-tokens"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/api-tokens", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });
}
