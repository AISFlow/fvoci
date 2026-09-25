import { useParams } from "react-router-dom";
import { t } from "@fvoci/i18n";
import { DocumentPage } from "@/pages/DocumentPage";
import { ProjectHomePage } from "@/pages/ProjectHomePage";
import { TaskDetailPage } from "@/pages/TaskDetailPage";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { parseRef } from "@/lib/href";
import "@/features/projects/projects.css";

export function WorkspaceRefPage() {
  const { ref } = useParams<{ ref: string }>();
  const { slug, workspace } = useWorkspaceContext();
  const parsed = parseRef(ref ?? "");

  if (parsed?.kind === "item" && parsed.prefix === "WIKI") {
    return <DocumentPage />;
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
