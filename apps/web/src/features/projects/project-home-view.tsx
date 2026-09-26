import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { childrenByParent, childrenOf } from "@/features/workspace/wiki-tree";
import { documentPath, projectTasksPath } from "@/lib/href";
import type { TreeNode } from "@/lib/queries/documents";
import type { Project } from "./queries";

function ProjectDocRow({
  slug,
  projectKey,
  node,
  nested = false,
}: {
  slug: string;
  projectKey: string;
  node: TreeNode;
  nested?: boolean;
}) {
  const ref = `${projectKey}-${node.number}`;
  return (
    <Link
      to={documentPath(slug, ref)}
      className={nested ? "wiki-tree__row wiki-tree__row--nested" : "wiki-tree__row"}
      data-testid={`project-doc-${ref}`}
    >
      {node.icon ? <span className="wiki-tree__icon" aria-hidden>{node.icon}</span> : null}
      <span className="wiki-tree__title">{node.title}</span>
      <span className="wiki-tree__key">{ref}</span>
    </Link>
  );
}

function ProjectDocBranch({
  slug,
  projectKey,
  node,
  byParent,
  nested = false,
}: {
  slug: string;
  projectKey: string;
  node: TreeNode;
  byParent: ReturnType<typeof childrenByParent<TreeNode>>;
  nested?: boolean;
}) {
  const childNodes = childrenOf(byParent, node.id);
  return (
    <li className={nested ? undefined : "wiki-tree__branch"}>
      <ProjectDocRow slug={slug} projectKey={projectKey} node={node} nested={nested} />
      {childNodes.length > 0 ? (
        <ul className="wiki-tree wiki-tree--nested">
          {childNodes.map((child) => (
            <ProjectDocBranch
              key={child.id}
              slug={slug}
              projectKey={projectKey}
              node={child}
              byParent={byParent}
              nested
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

export function ProjectHomeView({
  slug,
  project,
  nodes,
  loading,
  error,
  creating,
  onRetry,
  onCreateDocument,
  canManage = false,
  lifecyclePending = false,
  lifecycleError = null,
  onLifecycle,
}: {
  slug: string;
  project: Project;
  nodes: TreeNode[];
  loading: boolean;
  error: string | null;
  creating: boolean;
  onRetry: () => void;
  onCreateDocument: () => void;
  canManage?: boolean;
  lifecyclePending?: boolean;
  lifecycleError?: string | null;
  onLifecycle?: (action: "archive" | "unarchive" | "delete") => void;
}) {
  const childNodes = nodes.filter(
    (node) => node.parentId === project.rootDocumentId && node.projectId === project.id,
  );
  const byParent = childrenByParent(nodes);
  const archived = project.status === "archived";
  const canWrite = !archived && Boolean(project.rootDocumentId);

  return (
    <div className="project-home">
      <div className="project-home__head">
        <div className="project-home__crumb">
          <Link to={projectTasksPath(slug, project.key)}>{t("nav.tasks")}</Link>
          <span aria-hidden="true"> / </span>
          <span>{t("nav.wiki")}</span>
        </div>
        <h1 className="project-home__title">{project.name}</h1>
        <p className="project-home__meta">
          <span className="project-list__key">{project.key}</span>
          {project.visibility === "private" ? (
            <span className="project-list__private">{t("project.visibility.private")}</span>
          ) : null}
          {archived ? <span className="project-list__private">{t("project.archived.badge")}</span> : null}
        </p>
      </div>
      <div className="project-home__section-head">
        <h2 className="project-home__section-title">{t("nav.wiki")}</h2>
        {canWrite && childNodes.length > 0 ? (
          <Button type="button" variant="outline" disabled={creating} onClick={onCreateDocument}>
            {creating ? t("doc.create.pending") : t("nav.newDocument")}
          </Button>
        ) : null}
      </div>
      {loading ? <QueryLoading /> : null}
      {!loading && error ? <QueryError message={error} onRetry={onRetry} /> : null}
      {!loading && !error && childNodes.length === 0 ? (
        <EmptyState
          title={t("doc.empty")}
          description={archived ? undefined : t("doc.emptyHint")}
          actionLabel={canWrite ? t("nav.newDocument") : undefined}
          onAction={canWrite ? onCreateDocument : undefined}
        />
      ) : null}
      {canManage && onLifecycle ? (
        <div className="project-home__lifecycle flex flex-wrap gap-2" data-testid="project-lifecycle">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={lifecyclePending}
            onClick={() => {
              if (archived) {
                onLifecycle("unarchive");
                return;
              }
              if (window.confirm(`${t("project.archive.confirm.title")}\n${t("project.archive.confirm.body")}`)) {
                onLifecycle("archive");
              }
            }}
          >
            {archived ? t("project.unarchive") : t("project.archive")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={lifecyclePending}
            onClick={() => {
              if (window.confirm(`${t("project.delete.confirm.title")}\n${t("project.delete.confirm.body")}`)) {
                onLifecycle("delete");
              }
            }}
          >
            {t("project.delete")}
          </Button>
          {lifecycleError ? (
            <p role="alert" className="task-form__alert">{lifecycleError}</p>
          ) : null}
        </div>
      ) : null}
      {!loading && !error && childNodes.length > 0 ? (
        <ul className="wiki-tree">
          {childNodes.map((node) => (
            <ProjectDocBranch
              key={node.id}
              slug={slug}
              projectKey={project.key}
              node={node}
              byParent={byParent}
            />
          ))}
        </ul>
      ) : null}
    </div>
  );
}
