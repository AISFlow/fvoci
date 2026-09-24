import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { findProjectByKey, projectsQuery, workflowQuery } from "@/features/projects/queries";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import {
  eligibleParentCandidates,
  isRecurrenceKind,
  patchDateBody,
  patchEstimateBody,
  patchTitleBody,
  patchTypeBody,
  recurrenceBody,
  type PatchTaskBody,
} from "@/features/tasks/task-edit-payload";
import { taskFieldValidationMessage, taskMutationErrorMessage } from "@/features/tasks/task-errors";
import { TaskDetailView } from "@/features/tasks/task-detail";
import { lookupQuery, resolveLookupTarget } from "@/features/tasks/lookup";
import { taskListQuery, taskQuery } from "@/features/tasks/queries";
import { mergeTaskListPages } from "@/features/tasks/task-list-page";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { parseRef, projectsPath, projectTasksPath } from "@/lib/href";
import "@/features/projects/projects.css";

export function TaskDetailPage() {
  const { ref } = useParams<{ ref: string }>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");
  const item = parsed?.kind === "item" && parsed.prefix !== "WIKI" ? parsed : null;
  const displayId = item?.displayId ?? "";
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const project = findProjectByKey(projects.data?.items, item?.prefix ?? "");
  const lookup = useQuery(lookupQuery(workspace?.id ?? "", displayId));
  const lookupTarget = lookup.isSuccess
    ? resolveLookupTarget(lookup.data.items, displayId)
    : null;
  const lookupTask = lookupTarget?.kind === "task" ? lookupTarget.item : null;
  const projectDocument =
    lookupTarget?.kind === "project-document" ? lookupTarget.item : null;
  const task = useQuery(taskQuery(workspace?.id ?? "", lookupTask?.id ?? ""));
  const workflow = useQuery(
    workflowQuery(workspace?.id ?? "", project?.id ?? lookupTask?.projectId ?? ""),
  );
  const taskPages = useInfiniteQuery(
    taskListQuery(workspace?.id ?? "", project?.id ?? task.data?.projectId ?? ""),
  );
  const parentCandidates = task.data
    ? eligibleParentCandidates(task.data, mergeTaskListPages(taskPages.data?.pages ?? [])?.items ?? [])
    : [];

  const workspaceId = workspace?.id ?? "";
  const taskId = task.data?.id ?? "";
  const projectId = project?.id ?? task.data?.projectId ?? "";

  const afterMutation = async () => {
    await invalidateTaskCaches(queryClient, workspaceId, projectId, taskId);
  };

  const patchTask = useMutation({
    mutationFn: async (body: PatchTaskBody) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
          body,
        }),
      ),
    onSuccess: async () => {
      setFieldError(null);
      setActionError(null);
      await afterMutation();
    },
    onError: (err) => {
      setActionError(taskMutationErrorMessage(err));
    },
  });

  const moveTask = useMutation({
    mutationFn: async (input: { statusId: string; expectedStatusId: string }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
          body: {
            statusId: input.statusId,
            expectedStatusId: input.expectedStatusId,
          },
        }),
      ),
    onSuccess: async () => {
      setFieldError(null);
      setActionError(null);
      await afterMutation();
    },
    onError: (err) => {
      setActionError(taskMutationErrorMessage(err, "board.move.failed"));
    },
  });

  const trashTask = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
        }),
      ),
    onSuccess: async () => {
      setActionError(null);
      await invalidateTaskCaches(queryClient, workspaceId, projectId, taskId);
      const projectKey = project?.key ?? item?.prefix ?? "";
      if (projectKey) {
        await navigate(projectTasksPath(slug, projectKey));
      }
    },
    onError: (err) => {
      setActionError(taskMutationErrorMessage(err, "task.trash.failed"));
    },
  });

  const pending =
    patchTask.isPending || moveTask.isPending || trashTask.isPending;

  const runPatch = async (body: PatchTaskBody) => {
    if (!task.data?.canEdit) return;
    setActionError(null);
    await patchTask.mutateAsync(body);
  };

  if (!workspace) return null;

  const projectsDenied =
    projects.isError &&
    projects.error instanceof ProblemError &&
    projects.error.status === 404;
  const missingItem = item == null;
  const lookup404 =
    lookup.isError && lookup.error instanceof ProblemError && lookup.error.status === 404;
  const lookupMiss = lookup.isSuccess && lookupTarget?.kind === "miss";
  const task404 =
    Boolean(lookupTask) &&
    task.isError &&
    task.error instanceof ProblemError &&
    task.error.status === 404;
  const realNotFound = missingItem || projectsDenied || lookup404 || lookupMiss || task404;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {projects.isLoading ? <QueryLoading /> : null}
      {projects.isError && !projectsDenied ? (
        <QueryError
          message={loadErrorMessage(projects.error)}
          onRetry={() => {
            void projects.refetch();
          }}
        />
      ) : null}
      {realNotFound ? (
        <p role="alert" className="task-form__alert">
          {t("task.error.notFound")}
        </p>
      ) : null}
      {projectDocument ? (
        <p className="task-home__note">{t("task.document.unsupported")}</p>
      ) : null}
      {lookup.isLoading ? <QueryLoading /> : null}
      {lookup.isError && !lookup404 ? (
        <QueryError
          message={loadErrorMessage(lookup.error)}
          onRetry={() => {
            void lookup.refetch();
          }}
        />
      ) : null}
      {lookupTask && task.isLoading ? <QueryLoading /> : null}
      {lookupTask && task.isError && !task404 ? (
        <QueryError
          message={loadErrorMessage(task.error)}
          onRetry={() => {
            void task.refetch();
          }}
        />
      ) : null}
      {lookupTask && workflow.isError ? (
        <QueryError
          message={loadErrorMessage(workflow.error)}
          onRetry={() => {
            void workflow.refetch();
          }}
        />
      ) : null}
      {item && task.data ? (
        <TaskDetailView
          slug={slug}
          projectKey={project?.key ?? item.prefix}
          projectName={project?.name}
          task={task.data}
          statuses={workflow.data?.statuses ?? []}
          parentCandidates={parentCandidates}
          readOnly={!task.data.canEdit || task.data.archivedAt !== null}
          canEdit={task.data.canEdit}
          pending={pending}
          fieldError={fieldError}
          actionError={actionError}
          archivePending={patchTask.isPending}
          trashPending={trashTask.isPending}
          onTitleBlur={async (title) => {
            const parsedTitle = patchTitleBody(title);
            if (!parsedTitle.ok) {
              setFieldError(taskFieldValidationMessage(parsedTitle.issue));
              return;
            }
            await runPatch(parsedTitle.body);
          }}
          onStatusChange={async (statusId) => {
            if (!task.data || statusId === task.data.statusId) return;
            setActionError(null);
            await moveTask.mutateAsync({
              statusId,
              expectedStatusId: task.data.statusId,
            });
          }}
          onPriorityChange={async (priority) => {
            if (!task.data || priority === task.data.priority) return;
            await runPatch({ priority });
          }}
          onTypeChange={async (type) => {
            if (!task.data || type === task.data.type) return;
            const parsedType = patchTypeBody(type, task.data.parentId);
            if (!parsedType.ok) {
              setFieldError(taskFieldValidationMessage(parsedType.issue));
              return;
            }
            await runPatch(parsedType.body);
          }}
          onParentChange={async (parentId) => {
            if (!task.data || parentId === task.data.parentId) return;
            if (task.data.type === "subtask" && parentId === null) {
              setFieldError(taskFieldValidationMessage("parent"));
              return;
            }
            await runPatch({ parentId });
          }}
          onStartDateBlur={async (value) => {
            if (!task.data) return;
            const parsedDate = patchDateBody(task.data, "startDate", value);
            if (!parsedDate.ok) {
              setFieldError(taskFieldValidationMessage(parsedDate.issue));
              return;
            }
            await runPatch(parsedDate.body);
          }}
          onDueDateBlur={async (value) => {
            if (!task.data) return;
            const parsedDate = patchDateBody(task.data, "dueDate", value);
            if (!parsedDate.ok) {
              setFieldError(taskFieldValidationMessage(parsedDate.issue));
              return;
            }
            await runPatch(parsedDate.body);
          }}
          onEstimateBlur={async (value) => {
            const parsedEstimate = patchEstimateBody(value);
            if (!parsedEstimate.ok) {
              setFieldError(taskFieldValidationMessage(parsedEstimate.issue));
              return;
            }
            await runPatch(parsedEstimate.body);
          }}
          onRecurrenceChange={async (kind) => {
            const next = kind === "" ? null : isRecurrenceKind(kind) ? kind : null;
            const current = task.data?.recurrence;
            const currentKind =
              current && typeof current === "object" && "kind" in current
                ? String((current as { kind: unknown }).kind)
                : "";
            if ((next ?? "") === currentKind) return;
            await runPatch({ recurrence: recurrenceBody(next) });
          }}
          onArchiveToggle={async (archived) => {
            await runPatch({ archived });
          }}
          onTrash={async () => {
            if (!window.confirm(`${t("task.trash.confirm.title")}\n${t("task.trash.confirm.body")}`)) {
              return;
            }
            await trashTask.mutateAsync();
          }}
        />
      ) : null}
      {realNotFound || projectDocument ? (
        <p className="task-home__note">
          <Link to={project ? projectTasksPath(slug, project.key) : projectsPath(slug)}>
            {t("nav.projects")}
          </Link>
        </p>
      ) : null}
    </WorkspaceShell>
  );
}
