import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Navigate } from "react-router-dom";
import { useState } from "react";
import { WorkspaceGroupsSection } from "@/features/settings/workspace-groups";
import { WorkspaceIdentitySection } from "@/features/settings/workspace-identity";
import { WorkspaceMembersSection } from "@/features/settings/workspace-members";
import { WorkspaceTokensSection } from "@/features/settings/workspace-tokens";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";

function roleAtLeast(role: string, minimum: string): boolean {
  const order = ["guest", "member", "admin", "owner"];
  return order.indexOf(role) >= order.indexOf(minimum);
}

export function WorkspaceSettingsPage() {
  const queryClient = useQueryClient();
  const [nameError, setNameError] = useState<string | null>(null);
  const me = useQuery(meQuery);
  const { slug, workspace } = useWorkspaceContext();

  const metaQuery = useQuery({
    queryKey: ["workspaces", workspace?.id, "meta"],
    enabled: Boolean(workspace?.id),
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}", {
          params: { path: { workspace_id: workspace!.id } },
        }),
      ),
    retry: false,
  });

  const rename = useMutation({
    mutationFn: async (name: string) =>
      ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}", {
          params: { path: { workspace_id: workspace!.id } },
          body: { name },
        }),
      ),
    onSuccess: async () => {
      setNameError(null);
      await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
      await queryClient.invalidateQueries({ queryKey: ["workspaces", workspace?.id] });
    },
    onError: (err: unknown) => {
      setNameError(err instanceof ProblemError ? err.title : t("error.network"));
    },
  });

  if (me.isError) {
    return <Navigate to="/login" replace />;
  }

  if (!workspace) {
    return <Navigate to="/?denied=workspace" replace />;
  }

  if (metaQuery.isError) {
    return <Navigate to="/?denied=workspace" replace />;
  }

  const canManage = roleAtLeast(workspace.role, "admin");

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="settings"
    >
      <div className="settings-page">
        <WorkspaceIdentitySection
          workspaceName={metaQuery.data?.name ?? workspace.name}
          workspaceSlug={metaQuery.data?.slug ?? workspace.slug}
          workspaceKind={workspace.kind}
          canManage={canManage}
          namePending={rename.isPending}
          nameError={nameError}
          onSaveName={async (name) => {
            await rename.mutateAsync(name);
          }}
        />
        {roleAtLeast(workspace.role, "member") ? (
          <WorkspaceMembersSection
            workspaceId={workspace.id}
            currentUserId={me.data?.userId ?? null}
            currentUserRole={workspace.role}
          />
        ) : null}
        {roleAtLeast(workspace.role, "member") ? (
          <WorkspaceGroupsSection workspaceId={workspace.id} canManage={canManage} />
        ) : null}
        {canManage ? <WorkspaceTokensSection workspaceId={workspace.id} /> : null}
      </div>
    </WorkspaceShell>
  );
}
