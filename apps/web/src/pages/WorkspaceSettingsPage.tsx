import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useNavigate } from "react-router-dom";
import { useState } from "react";
import { WorkspaceGroupsSection } from "@/features/settings/workspace-groups";
import { WorkspaceIdentitySection } from "@/features/settings/workspace-identity";
import { WorkspaceMembersSection } from "@/features/settings/workspace-members";
import { WorkspaceCalendarSection } from "@/features/settings/workspace-calendar";
import { WorkspaceTokensSection } from "@/features/settings/workspace-tokens";
import { WorkspaceSsoSection } from "@/features/settings/workspace-sso";
import { WorkspaceWebhooksSection } from "@/features/settings/workspace-webhooks";
import { WorkspaceGithubSection } from "@/features/settings/workspace-github";
import { WorkspaceImportSection } from "@/features/settings/workspace-import";
import { NotificationPrefsSection } from "@/features/notifications/notification-prefs";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { documentTagsSettingsPath } from "@/lib/href";

function roleAtLeast(role: string, minimum: string): boolean {
  const order = ["guest", "member", "admin", "owner"];
  return order.indexOf(role) >= order.indexOf(minimum);
}

export function WorkspaceSettingsPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [nameError, setNameError] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
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

  const remove = useMutation({
    mutationFn: async (confirmSlug: string) =>
      ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}", {
          params: { path: { workspace_id: workspace!.id } },
          body: { confirmSlug },
        }),
      ),
    onSuccess: async () => {
      setDeleteError(null);
      // Leave `/w/:slug` before invalidating the workspace list. Invalidating
      // first makes WorkspaceLayout treat the deleted slug as denied.
      await navigate("/", { replace: true });
      await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
    },
    onError: (err: unknown) => {
      setDeleteError(err instanceof ProblemError ? err.title : t("error.network"));
    },
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
  const isOwner = workspace.role === "owner";

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="settings"
    >
      <div className="settings-page">
        <nav aria-label={t("nav.workspaceSettings")} className="mb-6 flex flex-wrap gap-3">
          <Link to={documentTagsSettingsPath(slug)} className="text-ui underline underline-offset-2">
            {t("settings.documentTags.nav")}
          </Link>
        </nav>
        <WorkspaceIdentitySection
          workspaceName={metaQuery.data?.name ?? workspace.name}
          workspaceSlug={metaQuery.data?.slug ?? workspace.slug}
          workspaceKind={workspace.kind}
          canManage={canManage}
          isOwner={isOwner}
          namePending={rename.isPending}
          nameError={nameError}
          onSaveName={async (name) => {
            await rename.mutateAsync(name);
          }}
          deletePending={remove.isPending}
          deleteError={deleteError}
          onDelete={async (confirmSlug) => {
            await remove.mutateAsync(confirmSlug);
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
        <WorkspaceImportSection workspaceId={workspace.id} canManage={canManage} />
        {roleAtLeast(workspace.role, "member") ? (
          <NotificationPrefsSection workspaceId={workspace.id} />
        ) : null}
        <WorkspaceCalendarSection workspaceId={workspace.id} />
        {canManage ? <WorkspaceTokensSection workspaceId={workspace.id} /> : null}
        {canManage ? <WorkspaceSsoSection workspaceId={workspace.id} /> : null}
        {canManage ? <WorkspaceWebhooksSection workspaceId={workspace.id} /> : null}
        {canManage ? <WorkspaceGithubSection workspaceId={workspace.id} /> : null}
      </div>
    </WorkspaceShell>
  );
}
