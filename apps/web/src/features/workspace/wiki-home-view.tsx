import { t, type I18nKey } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { documentPath, trashPath, wikiDisplayId } from "@/lib/href";
import type { TreeNode } from "@/lib/queries/documents";
import { childrenByParent, childrenOf } from "@/features/workspace/wiki-tree";

function roleLabel(role: string): string {
  const key = (
    {
      guest: "role.guest",
      member: "role.member",
      admin: "role.admin",
      owner: "role.owner",
    } as Record<string, I18nKey>
  )[role];
  return key ? t(key) : role;
}

function statusLabel(status: TreeNode["status"]): string | null {
  if (status === "draft") return t("doc.status.draft");
  if (status === "archived") return t("doc.status.archived");
  return null;
}

function WikiDocRow({
  slug,
  node,
  nested = false,
}: {
  slug: string;
  node: TreeNode;
  nested?: boolean;
}) {
  const status = statusLabel(node.status);
  const ref = wikiDisplayId(node.number);
  return (
    <Link
      to={documentPath(slug, ref)}
      className={nested ? "wiki-tree__row wiki-tree__row--nested" : "wiki-tree__row"}
      data-testid={`wiki-doc-${ref}`}
    >
      {node.icon ? (
        <span className="wiki-tree__icon" aria-hidden>{node.icon}</span>
      ) : null}
      <span className="wiki-tree__title">{node.title}</span>
      {status ? <span className="wiki-tree__status">{status}</span> : null}
      <span className="wiki-tree__key">{ref}</span>
    </Link>
  );
}

function WikiBranch({
  slug,
  node,
  byParent,
  nested = false,
}: {
  slug: string;
  node: TreeNode;
  byParent: ReturnType<typeof childrenByParent<TreeNode>>;
  nested?: boolean;
}) {
  const childNodes = childrenOf(byParent, node.id);
  return (
    <li className={nested ? undefined : "wiki-tree__branch"}>
      <WikiDocRow slug={slug} node={node} nested={nested} />
      {childNodes.length > 0 ? (
        <ul className="wiki-tree wiki-tree--nested">
          {childNodes.map((child) => (
            <WikiBranch key={child.id} slug={slug} node={child} byParent={byParent} nested />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

interface WikiHomeViewProps {
  slug: string;
  nodes: TreeNode[];
  loading: boolean;
  error: string | null;
  creating: boolean;
  createError: string | null;
  canCreate: boolean;
  role: string;
  onRetry: () => void;
  onCreate: () => void;
}

export function WikiHomeView({
  slug,
  nodes,
  loading,
  error,
  creating,
  createError,
  canCreate,
  role,
  onRetry,
  onCreate,
}: WikiHomeViewProps) {
  const wikiRoots = nodes.filter((node) => node.parentId === null && node.projectId === null);
  const byParent = childrenByParent(nodes);
  const empty = !loading && !error && wikiRoots.length === 0;

  return (
    <div className="wiki-home">
      <div className="wiki-home__head">
        <div className="wiki-home__intro">
          <h1 className="wiki-home__title">{t("nav.wiki")}</h1>
          <p className="wiki-home__role">{t("wiki.role.current", { role: roleLabel(role) })}</p>
          <Link className="wiki-home__trash-link" to={trashPath(slug)}>
            {t("trash.title")}
          </Link>
        </div>
        {canCreate && !empty ? (
          <Button type="button" disabled={creating} onClick={onCreate}>
            {creating ? t("doc.create.pending") : t("nav.newDocument")}
          </Button>
        ) : null}
      </div>
      {createError ? (
        <p role="alert" className="wiki-home__error">{createError}</p>
      ) : null}
      {empty ? (
        <EmptyState
          title={t("doc.empty")}
          description={canCreate ? t("doc.firstHint") : undefined}
          actionLabel={canCreate ? t("nav.newDocument") : undefined}
          onAction={canCreate ? onCreate : undefined}
          actionDisabled={creating}
        />
      ) : (
        <section className="wiki-home__section">
          {loading ? <QueryLoading /> : null}
          {!loading && error ? <QueryError message={error} onRetry={onRetry} /> : null}
          {!loading && !error && wikiRoots.length === 0 ? (
            <p className="wiki-home__empty">{t("nav.wikiEmpty")}</p>
          ) : null}
          {!loading && !error && wikiRoots.length > 0 ? (
            <ul className="wiki-tree">
              {wikiRoots.map((node) => (
                <WikiBranch key={node.id} slug={slug} node={node} byParent={byParent} />
              ))}
            </ul>
          ) : null}
        </section>
      )}
    </div>
  );
}
