// Adapted from source apps/web/src/features/settings/settings-templates.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useForm } from "react-hook-form";
import { useNavigate } from "react-router-dom";
import { QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { projectsQuery } from "@/features/projects/queries";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import { itemPath } from "@/lib/href";
import { templatesQuery } from "@/lib/queries";
import type { components } from "@/generated/api";
import { z } from "zod";
import "./settings-shell.css";
import "@/features/collections/collections.css";

type TemplateOutput = components["schemas"]["TemplateOutput"];
type TemplateCreateBody = components["schemas"]["TemplateCreateBody"];

const templateCreateFields = z.object({
  kind: z.enum(["document", "task"]),
  title: z.string().trim().min(1).max(200),
});

type TemplateCreateFields = z.infer<typeof templateCreateFields>;

export type TemplateApplyInput = {
  id: string;
  kind: TemplateOutput["kind"];
  projectId?: string;
};

const selectClass =
  "h-11 min-w-28 rounded-md border border-input bg-background px-3 text-ui";

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

function TemplateCreateForm({
  pending,
  onCreate,
}: {
  pending: boolean;
  onCreate: (input: TemplateCreateBody) => Promise<void>;
}) {
  const id = useId();
  const form = useForm<TemplateCreateFields>({
    resolver: zodResolver(templateCreateFields),
    defaultValues: { kind: "document", title: "" },
  });
  const titleError = formFieldMessage(form.formState.errors.title, "title");

  return (
    <form
      className="flex flex-col gap-2"
      noValidate
      onSubmit={form.handleSubmit((values) => {
        void Promise.resolve(
          onCreate({
            kind: values.kind,
            title: values.title,
            payload: { title: values.title },
          }),
        ).then(() => {
          form.reset({ kind: values.kind, title: "" });
        });
      })}
    >
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-title`}>{t("template.title")}</Label>
        <Input
          id={`${id}-title`}
          disabled={pending}
          aria-invalid={titleError ? true : undefined}
          {...form.register("title")}
        />
        {titleError ? (
          <p className="text-destructive" role="alert">
            {titleError}
          </p>
        ) : null}
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${id}-kind`}>{t("template.kind")}</Label>
        <select
          id={`${id}-kind`}
          className={selectClass}
          disabled={pending}
          aria-label={t("template.kind")}
          {...form.register("kind")}
        >
          <option value="document">{t("template.kind.document")}</option>
          <option value="task">{t("template.kind.task")}</option>
        </select>
      </div>
      <Button type="submit" size="sm" className="w-fit" disabled={pending}>
        {t("template.create")}
      </Button>
    </form>
  );
}

export function WorkspaceTemplatesSection({
  workspaceId,
  slug,
}: {
  workspaceId: string;
  slug: string;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const projectFieldId = useId();
  const [projectId, setProjectId] = useState("none");
  const [actionError, setActionError] = useState<string | null>(null);
  const listQuery = useQuery(templatesQuery(workspaceId));
  const projectsQueryResult = useQuery(projectsQuery(workspaceId));

  async function invalidate() {
    await queryClient.invalidateQueries({ queryKey: ["templates", workspaceId] });
  }

  const create = useMutation({
    mutationFn: async (input: TemplateCreateBody) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/templates", {
          params: { path: { workspace_id: workspaceId } },
          body: input,
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await invalidate();
    },
    onError: (err: unknown) => setActionError(failMessage(err)),
  });

  const apply = useMutation({
    mutationFn: async (input: TemplateApplyInput) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/templates/{template_id}/apply", {
          params: { path: { workspace_id: workspaceId, template_id: input.id } },
          body:
            input.kind === "task" && input.projectId && input.projectId !== "none"
              ? { projectId: input.projectId }
              : {},
        }),
      ),
    onSuccess: async (applied) => {
      setActionError(null);
      await navigate(itemPath(slug, applied.displayId));
    },
    onError: (err: unknown) => setActionError(failMessage(err)),
  });

  const pending = create.isPending || apply.isPending;
  const templates = listQuery.data?.items ?? [];
  const projects = projectsQueryResult.data?.items ?? [];
  const loading = listQuery.isLoading;
  const error = actionError ?? (listQuery.error ? failMessage(listQuery.error) : null);

  return (
    <div className="settings-stack">
      <section className="settings-section">
        <h2 className="settings-section__title text-title">{t("settings.templates")}</h2>
        <div className="flex flex-col gap-4">
          <TemplateCreateForm
            pending={pending}
            onCreate={async (input) => {
              await create.mutateAsync(input);
            }}
          />
          {error ? (
            <p className="text-ui text-destructive" role="alert">
              {error}
            </p>
          ) : null}
          {loading ? <QueryLoading /> : null}
          {!loading && templates.length === 0 ? (
            <p className="text-ui text-muted-foreground">{t("template.empty")}</p>
          ) : null}
          {templates.some((row) => row.kind === "task") ? (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor={projectFieldId}>{t("template.project")}</Label>
              <select
                id={projectFieldId}
                className={selectClass}
                aria-label={t("template.project")}
                value={projectId}
                disabled={pending}
                onChange={(event) => setProjectId(event.target.value)}
              >
                <option value="none">{t("template.project.placeholder")}</option>
                {projects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            </div>
          ) : null}
          {templates.length > 0 ? (
            <div className="data-table-wrap">
              <table className="data-table">
                <thead>
                  <tr>
                    <th scope="col">{t("template.title")}</th>
                    <th scope="col">{t("template.kind")}</th>
                    <th scope="col">
                      <span className="sr-only">{t("template.apply")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {templates.map((row) => {
                    const taskBlocked = row.kind === "task" && projectId === "none";
                    return (
                      <tr key={row.id}>
                        <td className="text-ui">{row.title}</td>
                        <td className="text-ui">
                          {row.kind === "task"
                            ? t("template.kind.task")
                            : t("template.kind.document")}
                        </td>
                        <td className="text-right">
                          <Button
                            type="button"
                            size="sm"
                            variant="outline"
                            disabled={pending || taskBlocked}
                            onClick={() => {
                              void apply.mutateAsync({
                                id: row.id,
                                kind: row.kind,
                                projectId: row.kind === "task" ? projectId : undefined,
                              });
                            }}
                          >
                            {t("template.apply")}
                          </Button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : null}
        </div>
      </section>
    </div>
  );
}
