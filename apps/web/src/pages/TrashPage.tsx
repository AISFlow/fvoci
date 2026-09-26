import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import { wikiPath } from "@/lib/href";
import { trashQuery } from "@/lib/queries/documents";
import { projectsQuery } from "@/features/projects/queries";

export function TrashPage() {
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const trash = useQuery(trashQuery(workspace?.id ?? ""));
  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const projectKeys = new Map(
    (projects.data?.items ?? []).map((project) => [project.id, project.key] as const),
  );
  const [restoreError, setRestoreError] = useState<string | null>(null);

  const restore = useMutation({
    mutationFn: async (item: { id: string; projectId?: string | null }) =>
      item.projectId
        ? ensureOk(
            await api.POST(
              "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/restore",
              {
                params: {
                  path: {
                    workspace_id: workspace!.id,
                    project_id: item.projectId,
                    document_id: item.id,
                  },
                },
              },
            ),
          )
        : ensureOk(
            await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/restore", {
              params: {
                path: { workspace_id: workspace!.id, document_id: item.id },
              },
            }),
          ),
    onSuccess: async (_data, item) => {
      setRestoreError(null);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["trash", workspace?.id] }),
        queryClient.invalidateQueries({ queryKey: ["tree", workspace?.id] }),
        item.projectId
          ? queryClient.invalidateQueries({
              queryKey: ["project-documents", workspace?.id, item.projectId],
            })
          : Promise.resolve(),
      ]);
    },
    onError: (error: unknown) => {
      setRestoreError(loadErrorMessage(error));
    },
  });

  if (!workspace) return null;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="wiki"
    >
      <div className="trash-page">
        <div className="trash-page__head">
          <h1 className="trash-page__title">{t("trash.title")}</h1>
          <Link to={wikiPath(slug)}>{t("nav.toWiki")}</Link>
        </div>
        <p className="trash-page__note">{t("doc.trash.retention")}</p>
        {restoreError ? (
          <p role="alert" className="trash-page__error">
            {restoreError}
          </p>
        ) : null}
        {trash.isLoading ? <QueryLoading /> : null}
        {trash.isError ? (
          <QueryError
            message={loadErrorMessage(trash.error)}
            onRetry={() => {
              void trash.refetch();
            }}
          />
        ) : null}
        {!trash.isLoading && !trash.isError && trash.data?.items.length === 0 ? (
          <p>{t("trash.empty")}</p>
        ) : null}
        {!trash.isLoading && !trash.isError && trash.data && trash.data.items.length > 0 ? (
          <ul className="trash-page__list">
            {trash.data.items.map((item) => (
              <li key={item.id} className="trash-page__row">
                <div className="trash-page__copy">
                  <span className="trash-page__name">{item.title}</span>
                  {item.projectId ? (
                    <span className="trash-page__project">
                      {projectKeys.get(item.projectId) ?? t("nav.projects")}
                    </span>
                  ) : null}
                  <time className="trash-page__when" dateTime={item.deletedAt}>
                    {new Date(item.deletedAt).toLocaleString()}
                  </time>
                </div>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={restore.isPending}
                  aria-label={`${t("trash.restore")} ${item.title}`}
                  onClick={() => {
                    setRestoreError(null);
                    restore.mutate({ id: item.id, projectId: item.projectId });
                  }}
                >
                  {t("trash.restore")}
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    </WorkspaceShell>
  );
}
