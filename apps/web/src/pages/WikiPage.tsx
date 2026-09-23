import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { WikiHomeView } from "@/features/workspace/wiki-home-view";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import { documentPath } from "@/lib/href";
import { loadErrorMessage } from "@/components/query-status";
import { treeQuery } from "@/lib/queries/documents";

export function WikiPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const tree = useQuery(treeQuery(workspace?.id ?? ""));

  const createDocument = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/documents", {
          params: { path: { workspace_id: workspace!.id } },
          body: { parentId: null, title: t("doc.title.untitled") },
        }),
      ),
    onSuccess: async (doc) => {
      await queryClient.invalidateQueries({ queryKey: ["tree", workspace?.id] });
      if (doc.displayId) {
        await navigate(documentPath(slug, doc.displayId));
      }
    },
  });

  if (!workspace) return null;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="wiki"
    >
      <WikiHomeView
        slug={slug}
        nodes={tree.data?.items ?? []}
        loading={tree.isLoading}
        error={tree.isError ? loadErrorMessage(tree.error) : null}
        creating={createDocument.isPending}
        onRetry={() => {
          void tree.refetch();
        }}
        onCreate={() => {
          createDocument.mutate();
        }}
      />
    </WorkspaceShell>
  );
}
