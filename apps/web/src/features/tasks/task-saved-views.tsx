// Adapted from source apps/web/src/features/tasks/task-saved-views.tsx plus the
// rename/delete controls of source project settings views (native dialog/select).
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { ConfirmActionButton } from "@/components/confirm-action";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeModal } from "@/features/projects/native-modal";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { asJsonObject, projectViewsQuery, type ProjectView } from "@/lib/queries/collections";
import { normalizeViewQuery, viewQueriesEqual, type ViewQuery } from "@/lib/view-query";
import "@/features/projects/projects.css";

export const PROJECT_VIEW_TYPES = ["list", "board", "calendar", "gantt", "table"] as const;
export type ProjectViewType = (typeof PROJECT_VIEW_TYPES)[number];

export function projectViewTypeLabel(type: string): string {
  switch (type) {
    case "list":
      return t("view.backlog");
    case "board":
      return t("view.board");
    case "calendar":
      return t("view.calendar");
    case "gantt":
      return t("view.gantt");
    case "table":
      return t("view.table");
    default:
      return type;
  }
}

export function viewConfigOf(view: ProjectView): ViewQuery {
  return normalizeViewQuery(view.config) ?? { filters: {}, sort: [] };
}

export function ProjectTaskSavedViews({
  workspaceId,
  projectId,
  query,
  selectedId,
  onSelect,
}: {
  workspaceId: string;
  projectId: string;
  query: ViewQuery;
  selectedId: string | null;
  /** Apply a saved view (its config becomes the current query), or clear the selection. */
  onSelect: (view: ProjectView | null) => void;
}) {
  const queryClient = useQueryClient();
  const selectId = useId();
  const dialogTitleId = useId();
  const nameId = useId();
  const typeId = useId();
  const renameId = useId();
  const viewsKey = projectViewsQuery(workspaceId, projectId).queryKey;
  const views = useQuery(projectViewsQuery(workspaceId, projectId));
  const items = views.data ?? [];
  const selected = items.find((view) => view.id === selectedId);
  const [createOpen, setCreateOpen] = useState(false);
  const [name, setName] = useState("");
  const [type, setType] = useState<ProjectViewType>("list");
  const [conflict, setConflict] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changed = selected !== undefined && !viewQueriesEqual(viewConfigOf(selected), query);

  function failure(err: unknown): string {
    return err instanceof ProblemError && err.titleKnown ? err.title : t("task.savedView.failed");
  }

  const create = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/views", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
          body: { name: name.trim(), type, config: asJsonObject(query) },
        }),
      ),
    onMutate: () => setError(null),
    onError: (err) => setError(failure(err)),
    onSuccess: async (created) => {
      queryClient.setQueryData(viewsKey, (previous: ProjectView[] = []) => [...previous, created]);
      await queryClient.invalidateQueries({ queryKey: viewsKey });
      setCreateOpen(false);
      setName("");
      setConflict(false);
      onSelect(created);
    },
  });

  const update = useMutation({
    mutationFn: async (input: { view: ProjectView; name?: string; config?: ViewQuery }) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/views/{view_id}", {
          params: { path: { workspace_id: workspaceId, view_id: input.view.id } },
          body: {
            ...(input.name !== undefined ? { name: input.name } : {}),
            ...(input.config !== undefined
              ? {
                  config: asJsonObject(input.config),
                  expectedConfig: asJsonObject(viewConfigOf(input.view)),
                }
              : {}),
          },
        }),
      ),
    onMutate: () => setError(null),
    onError: async (err) => {
      if (err instanceof ProblemError && err.status === 409) {
        const refreshed = await views.refetch();
        setConflict(refreshed.isSuccess);
        return;
      }
      setError(failure(err));
    },
    onSuccess: async () => {
      setConflict(false);
      await queryClient.invalidateQueries({ queryKey: viewsKey });
    },
  });

  const remove = useMutation({
    mutationFn: async (view: ProjectView) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/views/{view_id}", {
          params: { path: { workspace_id: workspaceId, view_id: view.id } },
        }),
      ),
    onMutate: () => setError(null),
    onError: (err) => setError(failure(err)),
    onSuccess: async (_ok, view) => {
      queryClient.setQueryData(viewsKey, (previous: ProjectView[] = []) =>
        previous.filter((item) => item.id !== view.id),
      );
      await queryClient.invalidateQueries({ queryKey: viewsKey });
      if (view.id === selectedId) onSelect(null);
    },
  });

  const pending = create.isPending || update.isPending || remove.isPending;

  return (
    <div className="flex flex-col gap-2" data-testid="task-saved-views">
      <div className="collection-toolbar">
        <div className="collection-field">
          <label htmlFor={selectId}>{t("project.views")}</label>
          <select
            id={selectId}
            className="collection-select"
            aria-label={t("task.savedView.select")}
            value={selected?.id ?? ""}
            disabled={views.isPending}
            onChange={(event) => {
              setConflict(false);
              setError(null);
              onSelect(items.find((view) => view.id === event.target.value) ?? null);
            }}
          >
            <option value="">{t("task.savedView.current")}</option>
            {items.map((view) => (
              <option key={view.id} value={view.id}>
                {view.name} · {projectViewTypeLabel(view.type)}
              </option>
            ))}
          </select>
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={pending}
          onClick={() => {
            create.reset();
            setError(null);
            setCreateOpen(true);
          }}
        >
          {t("task.savedView.create")}
        </Button>
        {changed ? (
          <>
            <span className="text-caption text-muted-foreground">{t("task.savedView.unsaved")}</span>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={pending || conflict}
              onClick={() => update.mutate({ view: selected, config: query })}
            >
              {t("task.savedView.update")}
            </Button>
          </>
        ) : null}
      </div>
      {selected ? (
        <div className="collection-toolbar">
          <form
            key={`${selected.id}:${selected.name}`}
            className="collection-toolbar"
            onSubmit={(event) => {
              event.preventDefault();
              const input = event.currentTarget.elements.namedItem("view-name");
              const next = input instanceof HTMLInputElement ? input.value.trim() : "";
              if (next === "" || next === selected.name) return;
              update.mutate({ view: selected, name: next });
            }}
          >
            <div className="collection-field">
              <label htmlFor={renameId}>{t("project.views.name")}</label>
              <Input
                id={renameId}
                name="view-name"
                className="h-9"
                maxLength={100}
                defaultValue={selected.name}
                disabled={pending}
              />
            </div>
            <Button type="submit" size="sm" variant="outline" disabled={pending}>
              {t("doc.tags.rename")}
            </Button>
          </form>
          <ConfirmActionButton
            title={t("collection.deleteView")}
            description={t("collection.deleteView.confirm")}
            actionLabel={t("project.views.delete")}
            disabled={pending}
            onConfirm={async () => {
              try {
                await remove.mutateAsync(selected);
              } catch {
                /* onError shows the message. */
              }
            }}
          >
            {t("collection.deleteView")}
          </ConfirmActionButton>
        </div>
      ) : null}
      {conflict && selected ? (
        <div role="alert" className="collection-toolbar">
          <span className="text-ui text-destructive">{t("task.savedView.conflict")}</span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => {
              setConflict(false);
              onSelect(selected);
            }}
          >
            {t("task.savedView.reload")}
          </Button>
        </div>
      ) : null}
      {views.isError ? (
        <div role="alert" className="collection-toolbar">
          <span className="text-ui text-destructive">{t("task.savedView.failed")}</span>
          <Button type="button" size="sm" variant="outline" onClick={() => void views.refetch()}>
            {t("task.savedView.retry")}
          </Button>
        </div>
      ) : null}
      {error && !createOpen ? (
        <p role="alert" className="text-ui text-destructive">
          {error}
        </p>
      ) : null}
      <NativeModal
        open={createOpen}
        labelledBy={dialogTitleId}
        onClose={() => setCreateOpen(false)}
      >
        <form
          className="task-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (!name.trim() || create.isPending) return;
            create.mutate();
          }}
        >
          <h2 id={dialogTitleId} className="project-dialog__title">
            {t("task.savedView.createTitle")}
          </h2>
          <p className="task-home__note">{t("task.savedView.createDescription")}</p>
          <div className="task-form__field">
            <Label htmlFor={nameId}>{t("task.savedView.name")}</Label>
            <Input
              id={nameId}
              value={name}
              maxLength={100}
              autoFocus
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <div className="task-form__field">
            <Label htmlFor={typeId}>{t("common.view")}</Label>
            <select
              id={typeId}
              className="collection-select"
              value={type}
              onChange={(event) => {
                const next = PROJECT_VIEW_TYPES.find((value) => value === event.target.value);
                if (next) setType(next);
              }}
            >
              {PROJECT_VIEW_TYPES.map((value) => (
                <option key={value} value={value}>
                  {projectViewTypeLabel(value)}
                </option>
              ))}
            </select>
          </div>
          {error ? (
            <p role="alert" className="task-form__alert">
              {error}
            </p>
          ) : null}
          <div className="task-form__actions">
            <Button type="button" variant="outline" onClick={() => setCreateOpen(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={create.isPending || !name.trim()}>
              {create.isPending ? t("task.savedView.creating") : t("task.savedView.createAction")}
            </Button>
          </div>
        </form>
      </NativeModal>
    </div>
  );
}
