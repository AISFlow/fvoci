import { t } from "@fvoci/i18n";
import { Link, Navigate } from "react-router-dom";
import { DocumentTagsSettingsSection } from "@/features/settings/settings-document-tags";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { settingsPath } from "@/lib/href";
import "@/features/projects/projects.css";

export function DocumentTagsSettingsPage() {
  const { slug, workspace } = useWorkspaceContext();
  if (!workspace) return <Navigate to="/?denied=workspace" replace />;
  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="settings"
    >
      <div className="settings-page">
        <p className="task-home__crumb">
          <Link to={settingsPath(slug)}>{t("nav.workspaceSettings")}</Link>
          <span aria-hidden="true"> / </span>
          <span>{t("settings.documentTags.nav")}</span>
        </p>
        <DocumentTagsSettingsSection workspaceId={workspace.id} />
      </div>
    </WorkspaceShell>
  );
}
