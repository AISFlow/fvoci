import { api, ensureOk } from "@/lib/api";
import type { ShareDocumentTarget, StarItem } from "@/lib/queries/share";

// Share link and star requests shared by the React and Vue document pages.

export async function createDocumentShareLink(
  workspaceId: string,
  target: ShareDocumentTarget,
  expiresInDays: number,
) {
  return ensureOk(
    target.projectId === null
      ? await api.POST("/api/v1/workspaces/{workspace_id}/documents/{id}/share-links", {
          params: { path: { workspace_id: workspaceId, id: target.documentId } },
          body: { expiresInDays },
        })
      : await api.POST(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{id}/share-links",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                project_id: target.projectId,
                id: target.documentId,
              },
            },
            body: { expiresInDays },
          },
        ),
  );
}

export async function revokeShareLink(workspaceId: string, id: string) {
  return ensureOk(
    await api.DELETE("/api/v1/workspaces/{workspace_id}/share-links/{id}", {
      params: { path: { workspace_id: workspaceId, id } },
    }),
  );
}

/** Removes `star` when set, otherwise stars the target (source cmdk star/unstar). */
export async function toggleStar(
  workspaceId: string,
  star: StarItem | undefined,
  type: "document" | "task",
  targetId: string,
) {
  if (star) {
    return ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/stars/{id}", {
        params: { path: { workspace_id: workspaceId, id: star.id } },
      }),
    );
  }
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/stars", {
      params: { path: { workspace_id: workspaceId } },
      body: { type, id: targetId },
    }),
  );
}

const SHARE_DATE_FORMAT = new Intl.DateTimeFormat("ko", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});

export function formatShareDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : SHARE_DATE_FORMAT.format(date);
}

export async function copyText(value: string): Promise<void> {
  if ("clipboard" in navigator && typeof navigator.clipboard.writeText === "function") {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}
