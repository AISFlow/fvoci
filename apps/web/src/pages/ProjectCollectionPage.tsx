// Source features/collections/project-collection-page.tsx: table/board/calendar
// routes over the project's task collection.
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { CollectionContents, type CollectionViewType } from "@/features/collections/collection-panel";
import { ProjectViewNav } from "@/features/collections/project-view-nav";
import { useProjectRef } from "@/features/collections/use-project-ref";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { projectCollectionPath, projectPath, projectsPath } from "@/lib/href";
import { projectCollectionQuery } from "@/lib/queries/collections";
import "@/features/projects/projects.css";

export function ProjectCollectionPage({ type }: { type: CollectionViewType }) {
  const navigate = useNavigate();
  const [search] = useSearchParams();
  const viewId = search.get("view");
  const { slug, workspace, projects, project, notFound } = useProjectRef();
  const collection = useQuery(projectCollectionQuery(workspace?.id ?? "", project?.id ?? ""));

  if (!workspace) return null;

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="projects"
    >
      {projects.isLoading ? <QueryLoading /> : null}
      {notFound ? (
        <p role="alert" className="task-form__alert">
          {t("project.notFound")}
        </p>
      ) : null}
      {!notFound && project ? (
        <div className="task-home">
          <p className="task-home__crumb">
            <Link to={projectsPath(slug)}>{t("nav.projects")}</Link>
            <span aria-hidden="true"> / </span>
            <Link to={projectPath(slug, project.key)}>{project.key}</Link>
          </p>
          <div className="task-home__head">
            <h1 className="task-home__title">{project.name}</h1>
          </div>
          <ProjectViewNav slug={slug} projectKey={project.key} active={type} />
          {collection.isPending ? <p role="status">{t("collection.loading")}</p> : null}
          {collection.isError ? (
            <QueryError
              message={loadErrorMessage(collection.error)}
              onRetry={() => void collection.refetch()}
            />
          ) : null}
          {collection.data ? (
            <CollectionContents
              key={`${collection.data.id}:${type}`}
              workspaceId={workspace.id}
              slug={slug}
              collectionId={collection.data.id}
              projectId={project.id}
              type={type}
              initialViewId={viewId}
              onOpenView={(nextType, nextViewId) => {
                if (nextType === type && nextViewId === viewId) return;
                const base = projectCollectionPath(slug, project.key, nextType);
                void navigate(nextViewId ? `${base}?view=${encodeURIComponent(nextViewId)}` : base, {
                  replace: nextType === type,
                });
              }}
            />
          ) : null}
        </div>
      ) : null}
    </WorkspaceShell>
  );
}
