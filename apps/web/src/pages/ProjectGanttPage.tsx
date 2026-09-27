import { useSearchParams } from "react-router-dom";
import { GanttProjectView } from "@/features/tasks/gantt-project-view";
import { useProjectRef } from "@/features/collections/use-project-ref";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { parseViewQueryParam } from "@/lib/view-query";
import { t } from "@fvoci/i18n";

export function ProjectGanttPage() {
  const [search] = useSearchParams();
  const { slug, workspace, project, notFound } = useProjectRef();
  const y = search.get("y");
  const m = search.get("m");
  const year = y ? Number.parseInt(y, 10) : undefined;
  const month = m ? Number.parseInt(m, 10) : undefined;
  const viewQuery = parseViewQueryParam(search.get("query"));

  if (!workspace) return null;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {notFound ? (
        <p role="alert" className="task-form__alert">{t("project.notFound")}</p>
      ) : project ? (
        <GanttProjectView
          workspaceId={workspace.id}
          slug={slug}
          projectId={project.id}
          projectKey={project.key}
          year={Number.isFinite(year) ? year : undefined}
          month={Number.isFinite(month) ? month : undefined}
          query={viewQuery ?? undefined}
        />
      ) : null}
    </WorkspaceShell>
  );
}
