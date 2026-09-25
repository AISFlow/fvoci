import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { findProjectByKey, projectsQuery, workflowQuery } from "@/features/projects/queries";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import {
  patchDateBody,
  patchTitleBody,
  patchTypeBody,
  type PatchTaskBody,
} from "@/features/tasks/task-edit-payload";
import { taskFieldValidationMessage, taskMutationErrorMessage } from "@/features/tasks/task-errors";
import { TaskDetailView } from "@/features/tasks/task-detail";
import { lookupQuery, resolveLookupTarget } from "@/features/tasks/lookup";
import { taskListQuery, taskQuery, projectLabelsQuery, projectMilestonesQuery } from "@/features/tasks/queries";
import { membersQuery } from "@/lib/queries";
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
  const [formEpoch, setFormEpoch] = useState(0);

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
  const parentItems = mergeTaskListPages(taskPages.data?.pages ?? [])?.items ?? [];
  const members = useQuery(membersQuery(workspace?.id ?? ""));
  const labels = useQuery(
    projectLabelsQuery(workspace?.id ?? "", project?.id ?? task.data?.projectId ?? ""),
  );
  const milestones = useQuery(
    projectMilestonesQuery(workspace?.id ?? "", project?.id ?? task.data?.projectId ?? ""),
  );

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

  const refetchAfterConflict = async (err: unknown) => {
    if (err instanceof ProblemError && err.status === 409) {
      await afterMutation();
      setFormEpoch((n) => n + 1);
    }
  };

  const runPatch = async (body: PatchTaskBody) => {
    if (!task.data?.canEdit) return;
    setFieldError(null);
    setActionError(null);
    try {
      await patchTask.mutateAsync(body);
    } catch (err) {
      await refetchAfterConflict(err);
    }
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
          parentItems={parentItems}
          members={members.data?.items ?? []}
          labels={labels.data?.items ?? []}
          milestones={milestones.data?.items ?? []}
          dependencyCandidates={parentItems.map((item) => ({
            id: item.id,
            number: item.number,
            title: item.title,
          }))}
          readOnly={!task.data.canEdit || task.data.archivedAt !== null}
          canEdit={task.data.canEdit}
          pending={pending}
          fieldError={fieldError}
          actionError={actionError}
          archivePending={patchTask.isPending}
          trashPending={trashTask.isPending}
          formEpoch={formEpoch}
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
            /* Source task-detail PATCHes statusId; collections and this rewrite use MOVE
             * with expectedStatusId so a stale status change 409s instead of overwriting. */
            try {
              await moveTask.mutateAsync({
                statusId,
                expectedStatusId: task.data.statusId,
              });
            } catch (err) {
              await refetchAfterConflict(err);
            }
          }}
          onPriorityChange={async (priority) => {
            if (!task.data || priority === task.data.priority) return;
            await runPatch({ priority });
          }}
          onHierarchySave={async (type, parentId) => {
            const parsedType = patchTypeBody(type, parentId);
            if (!parsedType.ok) {
              setFieldError(taskFieldValidationMessage(parsedType.issue));
              return;
            }
            await runPatch(parsedType.body);
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
          onAssigneesChange={async (assigneeIds) => {
            await runPatch({ assigneeIds });
          }}
          onLabelsChange={async (labelIds) => {
            await runPatch({ labelIds });
          }}
          onMilestoneChange={async (milestoneId) => {
            await runPatch({ milestoneId });
          }}
          onAddDependency={async (input) => {
            if (!task.data) return;
            setFieldError(null);
            setActionError(null);
            try {
              await ensureOk(
                await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies", {
                  params: { path: { workspace_id: workspaceId, task_id: task.data.id } },
                  body: input,
                }),
              );
              await afterMutation();
            } catch (err) {
              setActionError(taskMutationErrorMessage(err, "task.dep.add.failed"));
              await refetchAfterConflict(err);
              throw err;
            }
          }}
          onRemoveDependency={async (edge) => {
            setFieldError(null);
            setActionError(null);
            try {
              await ensureOk(
                await api.DELETE(
                  "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies/{blocked_id}",
                  {
                    params: {
                      path: {
                        workspace_id: workspaceId,
                        task_id: edge.blockerId,
                        blocked_id: edge.blockedId,
                      },
                    },
                  },
                ),
              );
              await afterMutation();
            } catch (err) {
              setActionError(taskMutationErrorMessage(err, "task.dep.remove.failed"));
            }
          }}
          onArchiveToggle={async (archived) => {
            await runPatch({ archived });
          }}
          onTrash={async () => {
            if (!window.confirm(`${t("task.trash.confirm.title")}\n${t("task.trash.confirm.body")}`)) {
              return;
            }
            try {
              await trashTask.mutateAsync();
            } catch {
              /* trashTask.onError already mapped the failure. */
            }
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
