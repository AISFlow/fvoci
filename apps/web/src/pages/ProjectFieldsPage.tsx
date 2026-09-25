// Source routes/w.$slug.$ref.settings.fields.tsx: the project task collection's fields.
import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { CollectionFieldManager } from "@/features/collections/field-manager";
import { ProjectViewNav } from "@/features/collections/project-view-nav";
import { useProjectRef } from "@/features/collections/use-project-ref";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { projectPath, projectsPath } from "@/lib/href";
import {
  collectionFieldsQuery,
  collectionPrefix,
  projectCollectionQuery,
} from "@/lib/queries/collections";
import "@/features/projects/projects.css";
import "@/features/settings/settings-shell.css";

export function ProjectFieldsPage() {
  const queryClient = useQueryClient();
  const { slug, workspace, projects, project, notFound } = useProjectRef();
  const workspaceId = workspace?.id ?? "";
  const collection = useQuery(projectCollectionQuery(workspaceId, project?.id ?? ""));
  const collectionId = collection.data?.id ?? "";
  const fields = useQuery(collectionFieldsQuery(workspaceId, collectionId));

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
          <ProjectViewNav slug={slug} projectKey={project.key} active="fields" />
          {collection.isPending || (collectionId && fields.isPending) ? <QueryLoading /> : null}
          {collection.isError || fields.isError ? (
            <QueryError
              message={loadErrorMessage(collection.error ?? fields.error)}
              onRetry={() => {
                void collection.refetch();
                void fields.refetch();
              }}
            />
          ) : null}
          {collection.data && fields.data ? (
            <div className="settings-stack mt-4">
              <CollectionFieldManager
                workspaceId={workspace.id}
                collectionId={collection.data.id}
                fields={fields.data.items}
                canManage={collection.data.canEdit && project.status === "active"}
                onSaved={async () => {
                  await queryClient.invalidateQueries({
                    queryKey: collectionPrefix(workspace.id, collection.data.id),
                  });
                }}
              />
            </div>
          ) : null}
        </div>
      ) : null}
    </WorkspaceShell>
  );
}
