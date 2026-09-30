import { useEffect } from "react";
import { useLocation, useParams } from "react-router-dom";
import { t } from "@fvoci/i18n";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { itemPath, projectPath, parseRef } from "@/lib/href";
import "@/features/projects/projects.css";

/**
 * Project and wiki resources are the Vue app's pages (src/app-boundary.ts). An in-app
 * link lands here first, as does a spelling the boundary does not send to
 * Vue but the router decodes to a resource ref (percent-encoded): load the
 * canonical path as a new page so the boot module starts the Vue app.
 */
function ResourceHandoff({ path }: { path: string }) {
  const { search, hash } = useLocation();
  useEffect(() => {
    window.location.replace(`${path}${search}${hash}`);
  }, [path, search, hash]);
  return null;
}

export function WorkspaceRefPage() {
  const { ref } = useParams<{ ref: string }>();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");

  if (parsed?.kind === "item") {
    return <ResourceHandoff path={itemPath(slug, parsed.displayId)} />;
  }
  if (parsed?.kind === "project") {
    return <ResourceHandoff path={projectPath(slug, parsed.key)} />;
  }
  if (!workspace) return null;
  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      <p role="alert" className="task-form__alert">
        {t("error.resource.notFound")}
      </p>
    </WorkspaceShell>
  );
}
