import { Navigate } from "react-router-dom";
import { WorkspaceTemplatesSection } from "@/features/settings/workspace-templates";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";

export function TemplatesSettingsPage() {
  const { slug, workspace } = useWorkspaceContext();

  if (!workspace) {
    return <Navigate to="/?denied=workspace" replace />;
  }

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="settings"
    >
      <WorkspaceTemplatesSection workspaceId={workspace.id} slug={slug} />
    </WorkspaceShell>
  );
}
