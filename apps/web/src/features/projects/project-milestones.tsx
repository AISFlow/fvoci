import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { projectMilestonesQuery } from "@/features/tasks/queries";
import { api, ensureOk, ProblemError } from "@/lib/api";
import "../settings/settings-shell.css";

export function ProjectMilestonesSection({
  workspaceId,
  projectId,
  canManage,
}: {
  workspaceId: string;
  projectId: string;
  canManage: boolean;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const milestonesQuery = useQuery(projectMilestonesQuery(workspaceId, projectId));
  const milestones = milestonesQuery.data?.items ?? [];

  async function refresh() {
    await queryClient.invalidateQueries({ queryKey: ["milestones", workspaceId, projectId] });
  }

  const create = useMutation({
    mutationFn: async (input: { name: string }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
          body: input,
        }),
      ),
    onSuccess: async () => {
      setError(null);
      setName("");
      await refresh();
    },
    onError: (err) => setError(err instanceof ProblemError ? err.title : t("error.network")),
  });
  const rename = useMutation({
    mutationFn: async (input: { milestoneId: string; name: string }) =>
      ensureOk(
        await api.PATCH(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                project_id: projectId,
                milestone_id: input.milestoneId,
              },
            },
            body: { name: input.name },
          },
        ),
      ),
    onSuccess: async () => {
      setError(null);
      await refresh();
    },
    onError: (err) => setError(err instanceof ProblemError ? err.title : t("error.network")),
  });
  const remove = useMutation({
    mutationFn: async (milestoneId: string) =>
      ensureOk(
        await api.DELETE(
          "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
          {
            params: {
              path: {
                workspace_id: workspaceId,
                project_id: projectId,
                milestone_id: milestoneId,
              },
            },
          },
        ),
      ),
    onSuccess: async () => {
      setError(null);
      await refresh();
      await queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] });
    },
    onError: (err) => setError(err instanceof ProblemError ? err.title : t("error.network")),
  });

  return (
    <section className="settings-section mt-6" data-testid="project-milestones">
      <h2 className="settings-section__title">{t("project.milestones")}</h2>
      <div className="flex flex-col gap-3">
        {milestonesQuery.isPending ? <p role="status">{t("load.loading")}</p> : null}
        {milestones.length === 0 && !milestonesQuery.isPending ? (
          <p className="task-home__note">{t("project.milestones.empty")}</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {milestones.map((row) => (
              <li key={row.id} className="flex items-center justify-between gap-2">
                {canManage ? (
                  <Input
                    defaultValue={row.name}
                    data-testid={`project-milestone-name-${row.id}`}
                    aria-label={t("project.milestones")}
                    disabled={rename.isPending}
                    onBlur={(event) => {
                      const next = event.currentTarget.value.trim();
                      if (next === "" || next === row.name) return;
                      void rename.mutateAsync({ milestoneId: row.id, name: next });
                    }}
                  />
                ) : (
                  <span data-testid={`project-milestone-${row.id}`}>{row.name}</span>
                )}
                {canManage ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    data-testid={`project-milestone-delete-${row.id}`}
                    disabled={remove.isPending}
                    onClick={() => void remove.mutateAsync(row.id)}
                  >
                    {t("project.milestones.delete")}
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
        )}
        {canManage ? (
          <form
            className="flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              if (!name.trim()) return;
              void create.mutateAsync({ name });
            }}
          >
            <div className="task-form__field">
              <Label htmlFor="project-milestone-name">{t("project.milestones")}</Label>
              <Input
                id="project-milestone-name"
                data-testid="project-milestone-name"
                aria-label={t("project.milestones")}
                disabled={create.isPending}
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </div>
            <Button type="submit" size="sm" data-testid="project-milestone-add" disabled={create.isPending}>
              {t("project.milestones.add")}
            </Button>
          </form>
        ) : null}
        {error ? (
          <p className="task-form__alert" role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </section>
  );
}
