import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { t } from "@fvoci/i18n";
import { loadErrorMessage } from "@/components/query-status";
import { ProjectHomeView } from "@/features/projects/project-home-view";
import {
  findProjectByKey,
  projectDocumentsQuery,
  projectQuery,
  projectsQuery,
} from "@/features/projects/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import { canonicalizeProjectKey, projectsPath } from "@/lib/href";

export function ProjectHomePage() {
  const { ref } = useParams<{ ref: string }>();
  const { slug, workspace } = useWorkspaceContext();
  const queryClient = useQueryClient();
  const projectKey = canonicalizeProjectKey(ref ?? "");
  const projects = useQuery(projectsQuery(workspace?.id ?? ""));
  const projectItem = findProjectByKey(projects.data?.items, projectKey);
  const project = useQuery(projectQuery(workspace?.id ?? "", projectItem?.id ?? ""));
  const documents = useQuery(
    projectDocumentsQuery(workspace?.id ?? "", projectItem?.id ?? ""),
  );
  const navigate = useNavigate();
  const [lifecycleError, setLifecycleError] = useState<string | null>(null);

  const lifecycle = useMutation({
    mutationFn: async (action: "archive" | "unarchive" | "delete") => {
      if (!workspace?.id || !projectItem?.id) throw new Error("missing project");
      const path = { workspace_id: workspace.id, project_id: projectItem.id };
      if (action === "delete") {
        return ensureOk(
          await api.DELETE("/api/v1/workspaces/{workspace_id}/projects/{project_id}", {
            params: { path },
          }),
        );
      }
      return ensureOk(
        action === "archive"
          ? await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/archive", {
              params: { path },
            })
          : await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/unarchive", {
              params: { path },
            }),
      );
    },
    onSuccess: async (_data, action) => {
      setLifecycleError(null);
      if (action === "delete") {
        await navigate(projectsPath(slug));
      }
      await queryClient.invalidateQueries({ queryKey: ["projects", workspace?.id] });
      await queryClient.invalidateQueries({ queryKey: ["project", workspace?.id, projectItem?.id] });
      await queryClient.invalidateQueries({ queryKey: ["trash", workspace?.id] });
    },
    onError: (error: unknown, action) => {
      setLifecycleError(
        action === "archive"
          ? t("project.archive.failed")
          : action === "unarchive"
            ? t("project.unarchive.failed")
            : loadErrorMessage(error),
      );
    },
  });

  const createDocument = useMutation({
    mutationFn: async () => {
      const rootId = project.data?.rootDocumentId;
      if (!workspace?.id || !projectItem?.id || !rootId) {
        throw new Error("missing project root");
      }
      return ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents", {
          params: {
            path: { workspace_id: workspace.id, project_id: projectItem.id },
          },
          body: {
            parentId: rootId,
            title: "새 문서",
          },
        }),
      );
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["project-documents", workspace?.id, projectItem?.id],
      });
    },
  });

  if (!workspace) return null;

  const loading = projects.isLoading || project.isLoading || documents.isLoading;
  const error =
    projects.isError || project.isError || documents.isError
      ? loadErrorMessage(projects.error ?? project.error ?? documents.error)
      : null;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {project.data ? (
        <ProjectHomeView
          slug={slug}
          project={project.data}
          nodes={documents.data?.items ?? []}
          loading={loading}
          error={error}
          creating={createDocument.isPending}
          onRetry={() => {
            void projects.refetch();
            void project.refetch();
            void documents.refetch();
          }}
          onCreateDocument={() => {
            void createDocument.mutate();
          }}
          canManage={projectItem?.canManage ?? false}
          lifecyclePending={lifecycle.isPending}
          lifecycleError={lifecycleError}
          onLifecycle={(action) => lifecycle.mutate(action)}
        />
      ) : loading ? null : (
        <p role="alert" className="task-form__alert">{error ?? "not found"}</p>
      )}
    </WorkspaceShell>
  );
}
