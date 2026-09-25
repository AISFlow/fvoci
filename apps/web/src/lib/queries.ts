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

export function groupsQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspaces", workspaceId, "groups"] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/groups", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
    enabled: Boolean(workspaceId),
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

export type NotificationFilter = "all" | "unread" | "archived";

export function notificationListQuery(
  workspaceId: string,
  filter: NotificationFilter,
  cursor?: string,
) {
  return queryOptions({
    queryKey: ["notifications", workspaceId, filter, cursor ?? ""] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/notifications", {
          params: {
            path: { workspace_id: workspaceId },
            query: {
              filter,
              ...(cursor ? { cursor } : {}),
            },
          },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export function notificationUnreadCountQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["notifications-unread", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/notifications/unread-count", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
    refetchInterval: 30_000,
  });
}

export function notificationPrefsQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["notification-prefs", workspaceId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/notification-prefs", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    enabled: Boolean(workspaceId),
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

export function workspaceWebhooksQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspaces", workspaceId, "webhooks"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/webhooks", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });
}

export function workspaceGithubQuery(workspaceId: string) {
  return queryOptions({
    queryKey: ["workspaces", workspaceId, "github"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/github", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });
}
