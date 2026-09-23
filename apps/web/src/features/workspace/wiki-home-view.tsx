import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { documentPath, wikiDisplayId } from "@/lib/href";
import type { TreeNode } from "@/lib/queries/documents";

function statusLabel(status: TreeNode["status"]): string | null {
  if (status === "draft") return t("doc.status.draft");
  if (status === "archived") return t("doc.status.archived");
  return null;
}

function WikiDocRow({ slug, node }: { slug: string; node: TreeNode }) {
  const status = statusLabel(node.status);
  const ref = wikiDisplayId(node.number);
  return (
    <Link
      to={documentPath(slug, ref)}
      className="wiki-tree__row"
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

function childrenByParent(nodes: readonly TreeNode[]): Map<string | null, TreeNode[]> {
  const index = new Map<string | null, TreeNode[]>();
  for (const node of nodes) {
    const bucket = index.get(node.parentId ?? null);
    if (bucket) bucket.push(node);
    else index.set(node.parentId ?? null, [node]);
  }
  return index;
}

interface WikiHomeViewProps {
  slug: string;
  nodes: TreeNode[];
  loading: boolean;
  error: string | null;
  creating: boolean;
  onRetry: () => void;
  onCreate: () => void;
}

export function WikiHomeView({
  slug,
  nodes,
  loading,
  error,
  creating,
  onRetry,
  onCreate,
}: WikiHomeViewProps) {
  const wikiRoots = nodes.filter((node) => node.parentId === null && node.projectId === null);
  const byParent = childrenByParent(nodes);
  const empty = !loading && !error && wikiRoots.length === 0;

  return (
    <div className="wiki-home">
      <div className="wiki-home__head">
        <h1 className="wiki-home__title">{t("nav.wiki")}</h1>
        {empty ? null : (
          <Button type="button" disabled={creating} onClick={onCreate}>
            {creating ? t("doc.create.pending") : t("nav.newDocument")}
          </Button>
        )}
      </div>
      {empty ? (
        <EmptyState
          title={t("doc.empty")}
          description={t("doc.firstHint")}
          actionLabel={t("nav.newDocument")}
          onAction={onCreate}
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
              {wikiRoots.map((node) => {
                const children = byParent.get(node.id) ?? [];
                return (
                  <li key={node.id} className="wiki-tree__branch">
                    <WikiDocRow slug={slug} node={node} />
                    {children.length > 0 ? (
                      <ul className="wiki-tree wiki-tree--nested">
                        {children.map((child) => (
                          <li key={child.id}>
                            <WikiDocRow slug={slug} node={child} />
                          </li>
                        ))}
                      </ul>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          ) : null}
        </section>
      )}
    </div>
  );
}
