import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { loadErrorMessage } from "@/components/query-status";
import { ProjectsView } from "@/features/projects/projects-view";
import { projectsQuery, type CreateProjectBody } from "@/features/projects/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import { membersQuery, meQuery } from "@/lib/queries";
import { projectPath } from "@/lib/href";
import type { CloneProjectBody } from "@/features/projects/queries";

export function ProjectsPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const listQuery = useQuery(projectsQuery(workspace?.id ?? ""));
  const members = useQuery(membersQuery(workspace?.id ?? ""));
  const me = useQuery(meQuery);

  const cloneProject = useMutation({
    mutationFn: async ({ projectId, body }: { projectId: string; body: CloneProjectBody }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/clone", {
          params: { path: { workspace_id: workspace!.id, project_id: projectId } },
          body,
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["projects", workspace?.id] });
    },
  });

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
        members={members.data?.items ?? []}
        currentUserId={me.data?.userId ?? null}
        loading={listQuery.isLoading}
        error={listQuery.isError ? loadErrorMessage(listQuery.error) : null}
        creating={createProject.isPending}
        cloning={cloneProject.isPending}
        onRetry={() => {
          void listQuery.refetch();
        }}
        onCreate={async (input) => {
          const project = await createProject.mutateAsync(input);
          await navigate(projectPath(slug, project.key));
        }}
        onClone={async (projectId, input) => {
          const project = await cloneProject.mutateAsync({ projectId, body: input });
          await navigate(projectPath(slug, project.key));
        }}
      />
    </WorkspaceShell>
  );
}
