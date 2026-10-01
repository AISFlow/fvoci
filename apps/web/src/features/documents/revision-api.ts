import { t } from "@fvoci/i18n";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

// Revision requests and labels shared by the React and Vue revision panels.

export type RevisionMeta = components["schemas"]["RevisionMetaResponse"];
export type RevisionDetail = components["schemas"]["RevisionDetailResponse"];

export const REASON_LABEL: Record<string, string> = {
  manual: t("version.reason.manual"),
  session: t("version.reason.session"),
  scheduled: t("version.reason.scheduled"),
};

export function formatAt(iso: string, timeZone: string): string {
  try {
    return new Intl.DateTimeFormat("ko", {
      timeZone,
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(iso));
  } catch {
    return iso;
  }
}

export function extractPreviewText(node: unknown): string {
  if (!node || typeof node !== "object") return "";
  const record = node as { text?: unknown; content?: unknown[] };
  if (typeof record.text === "string") return record.text;
  return (record.content ?? []).map(extractPreviewText).join("");
}

export function authorLabel(
  item: RevisionMeta,
  authorById: Readonly<Record<string, string>>,
): string {
  if (item.createdBy == null) {
    return t("version.author.system");
  }
  const name = authorById[item.createdBy];
  return name && name.trim().length > 0 ? name : t("version.author.member");
}

export type RevisionTargetKind = "document" | "task";

/* Source revision routes: documents and tasks share the revisions table and the
 * same list/create/get/restore contract under their own paths. Project
 * documents use the project-affiliated document paths (project permission). */
export async function listRevisions(
  kind: RevisionTargetKind,
  workspaceId: string,
  id: string,
  projectId: string | null,
  signal?: AbortSignal,
) {
  if (kind === "task") {
    return ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions", {
        signal,
        params: { path: { workspace_id: workspaceId, task_id: id }, query: { limit: 20 } },
      }),
    );
  }
  if (projectId) {
    return ensureOk(
      await api.GET(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions",
        {
          signal,
          params: {
            path: { workspace_id: workspaceId, project_id: projectId, document_id: id },
            query: { limit: 20 },
          },
        },
      ),
    );
  }
  return ensureOk(
    await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions", {
      signal,
      params: { path: { workspace_id: workspaceId, document_id: id }, query: { limit: 20 } },
    }),
  );
}

export async function createRevision(
  kind: RevisionTargetKind,
  workspaceId: string,
  id: string,
  projectId: string | null,
  signal?: AbortSignal,
) {
  if (kind === "task") {
    return ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions", {
        signal,
        params: { path: { workspace_id: workspaceId, task_id: id } },
      }),
    );
  }
  if (projectId) {
    return ensureOk(
      await api.POST(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions",
        {
          signal,
          params: { path: { workspace_id: workspaceId, project_id: projectId, document_id: id } },
        },
      ),
    );
  }
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions", {
      signal,
      params: { path: { workspace_id: workspaceId, document_id: id } },
    }),
  );
}

export async function restoreRevision(
  kind: RevisionTargetKind,
  workspaceId: string,
  id: string,
  projectId: string | null,
  revisionId: string,
  correlationId: string,
  signal?: AbortSignal,
) {
  if (kind === "task") {
    return ensureOk(
      await api.POST(
        "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}/restore",
        {
          signal,
          params: { path: { workspace_id: workspaceId, task_id: id, revision_id: revisionId } },
          body: { correlationId },
        },
      ),
    );
  }
  if (projectId) {
    return ensureOk(
      await api.POST(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions/{revision_id}/restore",
        {
          signal,
          params: {
            path: {
              workspace_id: workspaceId,
              project_id: projectId,
              document_id: id,
              revision_id: revisionId,
            },
          },
          body: { correlationId },
        },
      ),
    );
  }
  return ensureOk(
    await api.POST(
      "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore",
      {
        signal,
        params: {
          path: { workspace_id: workspaceId, document_id: id, revision_id: revisionId },
        },
        body: { correlationId },
      },
    ),
  );
}

export async function getRevision(
  kind: RevisionTargetKind,
  workspaceId: string,
  id: string,
  projectId: string | null,
  revisionId: string,
  signal?: AbortSignal,
) {
  if (kind === "task") {
    return ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}", {
        signal,
        params: { path: { workspace_id: workspaceId, task_id: id, revision_id: revisionId } },
      }),
    );
  }
  if (projectId) {
    return ensureOk(
      await api.GET(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions/{revision_id}",
        {
          signal,
          params: {
            path: {
              workspace_id: workspaceId,
              project_id: projectId,
              document_id: id,
              revision_id: revisionId,
            },
          },
        },
      ),
    );
  }
  return ensureOk(
    await api.GET(
      "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}",
      {
        signal,
        params: {
          path: { workspace_id: workspaceId, document_id: id, revision_id: revisionId },
        },
      },
    ),
  );
}
