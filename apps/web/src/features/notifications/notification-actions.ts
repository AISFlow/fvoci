import type { QueryClient } from "@tanstack/query-core";
import { api, ensureOk } from "@/lib/api";

// The notification writes the bell makes in both web apps, framework-neutral:
// each refreshes the inbox lists and the unread count afterwards.

export async function invalidateNotificationCaches(
  queryClient: QueryClient,
  workspaceId: string,
): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: ["notifications", workspaceId] });
  await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspaceId] });
}

/** Marks one notification read. */
export async function markNotificationRead(
  queryClient: QueryClient,
  workspaceId: string,
  id: string,
): Promise<void> {
  await ensureOk(
    await api.PATCH("/api/v1/workspaces/{workspace_id}/notifications/{id}", {
      params: { path: { workspace_id: workspaceId, id } },
      body: { read: true },
    }),
  );
  await invalidateNotificationCaches(queryClient, workspaceId);
}

/** Marks every notification of the workspace read. */
export async function markAllNotificationsRead(
  queryClient: QueryClient,
  workspaceId: string,
): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/notifications/read-all", {
      params: { path: { workspace_id: workspaceId } },
    }),
  );
  await invalidateNotificationCaches(queryClient, workspaceId);
}
