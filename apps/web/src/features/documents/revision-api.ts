import { t } from "@fvoci/i18n";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

// Revision transport for the existing Vue history panel.

export type RevisionMeta = components["schemas"]["RevisionMetaResponse"];
export type RevisionDetail = components["schemas"]["RevisionDetailResponse"];
export type RestorePreview = components["schemas"]["RevisionRestorePreviewResponse"];

export const REASON_LABEL: Record<string, string> = {
  manual: t("version.reason.manual"),
  session: t("version.reason.session"),
  scheduled: t("version.reason.scheduled"),
  restore: t("version.reason.restore"),
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

/** Bounded plain-text display; malformed/oversized trees are never rendered partially. */
export function extractPreviewText(node: unknown): string {
  const parts: string[] = [];
  const pending = [{ value: node, depth: 0 }];
  const seen = new Set<object>();
  let size = 0;
  while (pending.length) {
    const entry = pending.pop();
    if (!entry) break;
    if (
      !entry.value ||
      typeof entry.value !== "object" ||
      Array.isArray(entry.value) ||
      entry.depth > 64 ||
      seen.has(entry.value) ||
      seen.size >= 20_000
    )
      return "";
    seen.add(entry.value);
    const record = entry.value as {
      text?: unknown;
      content?: unknown;
      type?: unknown;
      attrs?: unknown;
    };
    if (typeof record.text === "string") {
      size += record.text.length;
      if (size > 2 * 1024 * 1024) return "";
      parts.push(record.text);
    }
    if (record.content !== undefined) {
      if (!Array.isArray(record.content)) return "";
      for (let index = record.content.length - 1; index >= 0; index--)
        pending.push({ value: record.content[index], depth: entry.depth + 1 });
    }
  }
  return parts.join("");
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
  expectedTailSeq: string,
  signal?: AbortSignal,
) {
  if (kind === "task") {
    return ensureOk(
      await api.POST(
        "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}/restore",
        {
          signal,
          params: { path: { workspace_id: workspaceId, task_id: id, revision_id: revisionId } },
          body: { correlationId, expectedTailSeq },
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
          body: { correlationId, expectedTailSeq },
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
        body: { correlationId, expectedTailSeq },
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

export async function previewRestoreRevision(
  kind: RevisionTargetKind,
  workspaceId: string,
  id: string,
  projectId: string | null,
  revisionId: string,
  signal?: AbortSignal,
): Promise<RestorePreview> {
  if (kind === "task")
    return ensureOk(
      await api.GET(
        "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}/restore-preview",
        {
          signal,
          params: { path: { workspace_id: workspaceId, task_id: id, revision_id: revisionId } },
        },
      ),
    );
  if (projectId)
    return ensureOk(
      await api.GET(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions/{revision_id}/restore-preview",
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
  return ensureOk(
    await api.GET(
      "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore-preview",
      {
        signal,
        params: { path: { workspace_id: workspaceId, document_id: id, revision_id: revisionId } },
      },
    ),
  );
}
