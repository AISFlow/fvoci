import { useEffect } from "react";
import { useLocation, useParams } from "react-router-dom";
import { t } from "@fvoci/i18n";
import { ProjectHomePage } from "@/pages/ProjectHomePage";
import { TaskDetailPage } from "@/pages/TaskDetailPage";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { documentPath, parseRef } from "@/lib/href";
import "@/features/projects/projects.css";

/**
 * Wiki documents are the Vue app's page (src/app-boundary.ts). An in-app
 * link lands here first, as does a spelling the boundary does not send to
 * Vue but the router decodes to a wiki ref (percent-encoded): load the
 * canonical path as a new page so the boot module starts the Vue app.
 */
function WikiDocumentHandoff({ path }: { path: string }) {
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

  if (parsed?.kind === "item" && parsed.prefix === "WIKI") {
    return <WikiDocumentHandoff path={documentPath(slug, parsed.displayId)} />;
  }
  if (parsed?.kind === "item") {
    return <TaskDetailPage />;
  }
  if (parsed?.kind === "project") {
    return <ProjectHomePage />;
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
