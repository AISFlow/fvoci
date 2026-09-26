// Source routes/w.$slug.$ref.settings.workflow.tsx + ProjectWorkflowSection:
// rename, recategorize, add and delete workflow statuses. The server decides
// manage permission; a refused write shows its problem title.
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { Link } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ProjectViewNav } from "@/features/collections/project-view-nav";
import { useProjectRef } from "@/features/collections/use-project-ref";
import { workflowQuery, type WorkflowStatus } from "@/features/projects/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { projectPath, projectsPath } from "@/lib/href";
import "@/features/projects/projects.css";
import "@/features/settings/settings-shell.css";

const STATUS_CATEGORIES = ["backlog", "todo", "in_progress", "done", "canceled"] as const;
type StatusCategory = (typeof STATUS_CATEGORIES)[number];

function categoryLabel(category: string): string {
  switch (category) {
    case "backlog":
      return t("seed.status.backlog");
    case "todo":
      return t("seed.status.todo");
    case "in_progress":
      return t("seed.status.in_progress");
    case "done":
      return t("seed.status.done");
    case "canceled":
      return t("seed.status.canceled");
    default:
      return category;
  }
}

function asCategory(value: string): StatusCategory {
  return STATUS_CATEGORIES.find((c) => c === value) ?? "todo";
}

function CategorySelect({
  value,
  disabled,
  onChange,
}: {
  value: StatusCategory;
  disabled: boolean;
  onChange: (value: StatusCategory) => void;
}) {
  return (
    <select
      className="h-10 rounded-md border border-border bg-background px-2 text-ui"
      aria-label={categoryLabel(value)}
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(asCategory(event.target.value))}
    >
      {STATUS_CATEGORIES.map((c) => (
        <option key={c} value={c}>
          {categoryLabel(c)}
        </option>
      ))}
    </select>
  );
}

function StatusRow({
  row,
  pending,
  readOnly,
  onPatch,
  onDelete,
}: {
  row: WorkflowStatus;
  pending: boolean;
  readOnly: boolean;
  onPatch: (id: string, patch: { name?: string; category?: StatusCategory }) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
}) {
  const [name, setName] = useState(row.name);
  const [category, setCategory] = useState<StatusCategory>(asCategory(row.category));
  return (
    <li className="flex flex-wrap items-end gap-2" data-testid={`workflow-status-${row.id}`}>
      <form
        className="flex min-w-0 flex-1 flex-wrap items-end gap-2"
        onSubmit={(event: FormEvent) => {
          event.preventDefault();
          const patch: { name?: string; category?: StatusCategory } = {};
          if (name.trim() !== row.name) patch.name = name.trim();
          if (category !== row.category) patch.category = category;
          if (Object.keys(patch).length === 0) return;
          void onPatch(row.id, patch);
        }}
      >
        <Input
          aria-label={t("project.workflow.statusName")}
          value={name}
          maxLength={100}
          disabled={pending || readOnly}
          onChange={(event) => setName(event.target.value)}
        />
        <CategorySelect value={category} disabled={pending || readOnly} onChange={setCategory} />
        {!readOnly ? (
          <Button type="submit" size="sm" disabled={pending || name.trim() === ""}>
            {t("project.workflow.saveStatus")}
          </Button>
        ) : null}
      </form>
      {!readOnly ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={pending}
          onClick={() => void onDelete(row.id)}
        >
          {t("project.workflow.deleteStatus")}
        </Button>
      ) : null}
    </li>
  );
}

export function ProjectWorkflowPage() {
  const queryClient = useQueryClient();
  const { slug, workspace, projects, project, notFound } = useProjectRef();
  const workspaceId = workspace?.id ?? "";
  const workflow = useQuery(workflowQuery(workspaceId, project?.id ?? ""));
  const [newName, setNewName] = useState("");
  const [newCategory, setNewCategory] = useState<StatusCategory>("todo");
  const [error, setError] = useState<string | null>(null);
  const workflowId = workflow.data?.id ?? "";

  const refresh = async () => {
    setError(null);
    await queryClient.invalidateQueries({ queryKey: ["workflow", workspaceId, project?.id ?? ""] });
    await queryClient.invalidateQueries({ queryKey: ["workspace-statuses", workspaceId] });
  };
  const onError = (err: unknown) => {
    setError(err instanceof ProblemError ? err.title : t("error.network"));
  };
  const createStatus = useMutation({
    mutationFn: async (body: { name: string; category: StatusCategory }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses", {
          params: { path: { workspace_id: workspaceId, workflow_id: workflowId } },
          body,
        }),
      ),
    onSuccess: async () => {
      setNewName("");
      setNewCategory("todo");
      await refresh();
    },
    onError,
  });
  const patchStatus = useMutation({
    mutationFn: async (input: { id: string; patch: { name?: string; category?: StatusCategory } }) =>
      ensureOk(
        await api.PATCH(
          "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
          {
            params: {
              path: { workspace_id: workspaceId, workflow_id: workflowId, status_id: input.id },
            },
            body: input.patch,
          },
        ),
      ),
    onSuccess: refresh,
    onError,
  });
  const deleteStatus = useMutation({
    mutationFn: async (id: string) =>
      ensureOk(
        await api.DELETE(
          "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
          { params: { path: { workspace_id: workspaceId, workflow_id: workflowId, status_id: id } } },
        ),
      ),
    onSuccess: refresh,
    onError,
  });

  if (!workspace) return null;
  const pending = createStatus.isPending || patchStatus.isPending || deleteStatus.isPending;
  const readOnly = project?.status !== "active";
  const statuses = workflow.data?.statuses ?? [];

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {projects.isLoading ? <QueryLoading /> : null}
      {notFound ? (
        <p role="alert" className="task-form__alert">
          {t("project.notFound")}
        </p>
      ) : null}
      {!notFound && project ? (
        <div className="task-home">
          <p className="task-home__crumb">
            <Link to={projectsPath(slug)}>{t("nav.projects")}</Link>
            <span aria-hidden="true"> / </span>
            <Link to={projectPath(slug, project.key)}>{project.key}</Link>
          </p>
          <div className="task-home__head">
            <h1 className="task-home__title">{project.name}</h1>
          </div>
          <ProjectViewNav slug={slug} projectKey={project.key} active="workflow" />
          <section className="settings-section mt-4" data-testid="project-workflow">
            <h2 className="settings-section__title text-title">{t("project.workflow")}</h2>
            <div className="flex flex-col gap-3">
              {workflow.isPending ? <QueryLoading /> : null}
              {workflow.isError ? (
                <QueryError
                  message={loadErrorMessage(workflow.error)}
                  onRetry={() => {
                    void workflow.refetch();
                  }}
                />
              ) : null}
              {workflow.isSuccess && statuses.length === 0 ? (
                <p className="text-ui text-muted-foreground">{t("project.workflow.empty")}</p>
              ) : null}
              {statuses.length > 0 ? (
                <ul className="flex flex-col gap-2">
                  {statuses.map((row) => (
                    <StatusRow
                      key={`${row.id}:${row.name}:${row.category}`}
                      row={row}
                      pending={pending}
                      readOnly={readOnly}
                      onPatch={async (id, patch) => {
                        await patchStatus.mutateAsync({ id, patch }).catch(() => undefined);
                      }}
                      onDelete={async (id) => {
                        await deleteStatus.mutateAsync(id).catch(() => undefined);
                      }}
                    />
                  ))}
                </ul>
              ) : null}
              {workflow.isSuccess && !readOnly ? (
                <form
                  className="flex flex-wrap items-end gap-2"
                  onSubmit={(event) => {
                    event.preventDefault();
                    const name = newName.trim();
                    if (name === "") return;
                    createStatus.mutate({ name, category: newCategory });
                  }}
                >
                  <Input
                    aria-label={t("project.workflow")}
                    value={newName}
                    maxLength={100}
                    disabled={pending}
                    onChange={(event) => setNewName(event.target.value)}
                  />
                  <CategorySelect value={newCategory} disabled={pending} onChange={setNewCategory} />
                  <Button type="submit" size="sm" disabled={pending || newName.trim() === ""}>
                    {t("project.workflow.add")}
                  </Button>
                </form>
              ) : null}
              {error ? (
                <p role="alert" className="text-ui text-destructive">
                  {error}
                </p>
              ) : null}
            </div>
          </section>
        </div>
      ) : null}
    </WorkspaceShell>
  );
}
