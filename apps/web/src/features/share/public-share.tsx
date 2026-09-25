import { asSafeHtml, SafeHtmlView } from "@fvoci/editor/safe-html";
import { t } from "@fvoci/i18n";
import { useLayoutEffect, useRef } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import type { ShareTreeNode } from "@/lib/queries/share";
import { isSafeShareHref, shareTreeChildren, shareTreeRoots } from "@/lib/share-links";
import "./share.css";

const dateFormat = new Intl.DateTimeFormat("ko", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});

function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : dateFormat.format(date);
}

function PublicTreeBranch({
  nodes,
  node,
  depth,
  activeDocumentId,
  onSelect,
}: {
  nodes: readonly ShareTreeNode[];
  node: ShareTreeNode;
  depth: number;
  activeDocumentId: string | null;
  onSelect: (documentId: string) => void;
}) {
  const children = shareTreeChildren(nodes, node.id);
  const isActive = node.id === activeDocumentId;
  return (
    <li>
      <button
        type="button"
        className={cn("share-page__tree-row", isActive && "is-active")}
        style={{ paddingLeft: `${depth * 0.75 + 0.375}rem` }}
        aria-current={isActive ? "page" : undefined}
        onClick={() => onSelect(node.id)}
      >
        {node.icon ? <span aria-hidden>{node.icon}</span> : null}
        <span className="min-w-0 truncate break-keep">{node.title}</span>
      </button>
      {children.length > 0 ? (
        <ul>
          {children.map((child) => (
            <PublicTreeBranch
              key={child.id}
              nodes={nodes}
              node={child}
              depth={depth + 1}
              activeDocumentId={activeDocumentId}
              onSelect={onSelect}
            />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

/**
 * Server `format=fragment` HTML is already sanitized; links still open detached.
 * Layout effect: links are hardened before the first paint, never clickable raw.
 */
function ShareBodyView({ html }: { html: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const root = ref.current;
    if (!root) return;
    for (const link of root.querySelectorAll("a")) {
      const href = link.getAttribute("href");
      if (href === null || !isSafeShareHref(href)) {
        link.removeAttribute("href");
        continue;
      }
      link.setAttribute("target", "_blank");
      link.setAttribute("rel", "noopener noreferrer");
    }
  }, [html]);
  return (
    <SafeHtmlView
      ref={ref}
      className="share-page__body"
      data-testid="share-body"
      html={asSafeHtml(html)}
    />
  );
}

export function PublicShareLoading() {
  return (
    <div className="share-page">
      <div className="share-page__gate">
        <p role="status" className="share-page__status">
          {t("doc.loading")}
        </p>
      </div>
    </div>
  );
}

export function PublicShareGate({ message }: { message: string }) {
  return (
    <div className="share-page">
      <div className="share-page__gate">
        <p role="alert" className="share-page__alert">
          {message}
        </p>
      </div>
    </div>
  );
}

export function PublicShareView({
  title,
  expiresAt,
  tree,
  activeDocumentId,
  onSelectDocument,
  body,
  bodyLoading,
  bodyError,
  onRetryBody,
}: {
  title: string;
  expiresAt: string;
  tree: readonly ShareTreeNode[];
  activeDocumentId: string | null;
  onSelectDocument: (documentId: string) => void;
  body: string | null;
  bodyLoading: boolean;
  bodyError: string | null;
  onRetryBody: () => void;
}) {
  const roots = shareTreeRoots(tree);
  const heading = tree.find((node) => node.id === activeDocumentId)?.title ?? title;
  const hasTree = tree.length > 1;

  return (
    <div className="share-page">
      <div className={cn("share-page__frame", hasTree ? null : "share-page__frame--solo")}>
        {hasTree ? (
          <aside className="share-page__rail">
            <nav aria-label={t("share.document")} className="share-page__tree">
              <ul>
                {roots.map((node) => (
                  <PublicTreeBranch
                    key={node.id}
                    nodes={tree}
                    node={node}
                    depth={0}
                    activeDocumentId={activeDocumentId}
                    onSelect={onSelectDocument}
                  />
                ))}
              </ul>
            </nav>
          </aside>
        ) : null}
        <main className="share-page__main">
          <div>
            <h1 className="share-page__heading">{heading}</h1>
            <p className="share-page__meta">
              <span className="share-page__badge">{t("doc.readOnly")}</span>
              <span>
                {t("share.expires")} {formatDate(expiresAt)}
              </span>
            </p>
          </div>
          {bodyLoading ? (
            <p role="status" className="share-page__status">
              {t("doc.loading")}
            </p>
          ) : null}
          {bodyError ? (
            <div className="flex flex-wrap items-center gap-3">
              <p role="alert" className="share-page__alert">
                {bodyError}
              </p>
              <Button type="button" size="sm" variant="outline" onClick={onRetryBody}>
                {t("load.retry")}
              </Button>
            </div>
          ) : null}
          {body !== null && !bodyLoading ? <ShareBodyView html={body} /> : null}
        </main>
      </div>
    </div>
  );
}
