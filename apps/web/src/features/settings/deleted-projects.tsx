import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { api, ensureOk } from "@/lib/api";
import "./settings-shell.css";

/** Source settings: workspace admins restore deleted projects (with their documents). */
export function DeletedProjectsSection({ workspaceId }: { workspaceId: string }) {
  const client = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const deleted = useQuery({
    queryKey: ["projects", workspaceId, "deleted"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects", {
          params: { path: { workspace_id: workspaceId }, query: { deleted: "true" } },
        }),
      ),
    retry: false,
  });
  const restore = useMutation({
    mutationFn: async (projectId: string) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/restore", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    onSuccess: async () => {
      setError(null);
      await Promise.all([
        client.invalidateQueries({ queryKey: ["projects", workspaceId] }),
        client.invalidateQueries({ queryKey: ["trash", workspaceId] }),
      ]);
    },
    onError: (err: unknown) => setError(loadErrorMessage(err)),
  });
  const items = deleted.data?.items ?? [];

  return (
    <section className="settings-section" aria-labelledby="deleted-projects-title">
      <h2 id="deleted-projects-title" className="settings-section__title">
        {t("project.restore")}
      </h2>
      {deleted.isLoading ? <QueryLoading /> : null}
      {deleted.isError ? (
        <QueryError
          message={loadErrorMessage(deleted.error)}
          onRetry={() => {
            void deleted.refetch();
          }}
        />
      ) : null}
      {!deleted.isLoading && !deleted.isError && items.length === 0 ? (
        <p className="settings-section__lede">{t("project.restore.empty")}</p>
      ) : null}
      {items.length > 0 ? (
        <ul className="flex flex-col gap-2" data-testid="deleted-projects">
          {items.map((project) => (
            <li key={project.id} className="flex items-center justify-between gap-2">
              <span>
                <span className="project-list__key">{project.key}</span> {project.name}
              </span>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={restore.isPending}
                aria-label={`${t("trash.restore")} ${project.name}`}
                onClick={() => {
                  if (
                    !window.confirm(
                      `${t("project.restore.confirm.title")}\n${t("project.restore.confirm.body")}`,
                    )
                  ) {
                    return;
                  }
                  restore.mutate(project.id);
                }}
              >
                {t("trash.restore")}
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
      {error ? (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      ) : null}
    </section>
  );
}
