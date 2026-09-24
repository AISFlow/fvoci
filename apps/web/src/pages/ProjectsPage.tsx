import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { loadErrorMessage } from "@/components/query-status";
import { ProjectsView } from "@/features/projects/projects-view";
import { projectsQuery, type CreateProjectBody } from "@/features/projects/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import { projectTasksPath } from "@/lib/href";

export function ProjectsPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const listQuery = useQuery(projectsQuery(workspace?.id ?? ""));

  const createProject = useMutation({
    mutationFn: async (body: CreateProjectBody) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects", {
          params: { path: { workspace_id: workspace!.id } },
          body,
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["projects", workspace?.id] });
    },
  });

  if (!workspace) return null;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      <ProjectsView
        slug={slug}
        projects={listQuery.data?.items ?? []}
        loading={listQuery.isLoading}
        error={listQuery.isError ? loadErrorMessage(listQuery.error) : null}
        creating={createProject.isPending}
        onRetry={() => {
          void listQuery.refetch();
        }}
        onCreate={async (input) => {
          const project = await createProject.mutateAsync(input);
          await navigate(projectTasksPath(slug, project.key));
        }}
      />
    </WorkspaceShell>
  );
}
