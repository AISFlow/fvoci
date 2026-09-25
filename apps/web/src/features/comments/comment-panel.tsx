import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, type ReactNode } from "react";
import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { ProblemError } from "@/lib/api";
import { commentsQuery, type CommentsTargetKind } from "@/lib/queries/comments";
import { CommentCompose, CommentItem, useCommentActions } from "./comment-actions";
import { buildCommentTree, type CommentNode } from "./comment-tree";
import "./comments.css";

interface CommentPanelProps {
  workspaceId: string;
  kind: CommentsTargetKind;
  targetId: string;
  currentUserId: string;
  readOnly?: boolean;
}

export function CommentPanel({
  workspaceId,
  kind,
  targetId,
  currentUserId,
  readOnly = false,
}: CommentPanelProps) {
  const queryClient = useQueryClient();
  const list = useInfiniteQuery(commentsQuery(workspaceId, kind, targetId));
  const actions = useCommentActions({
    workspaceId,
    kind,
    targetId,
    invalidate: () =>
      queryClient.invalidateQueries({ queryKey: ["comments", workspaceId, kind, targetId] }),
  });

  const items = useMemo(
    () => list.data?.pages.flatMap((page) => page.items) ?? [],
    [list.data],
  );
  const roots = useMemo(() => buildCommentTree(items), [items]);
  const testId = kind === "document" ? "document-comments" : "task-comments";

  function renderNode(node: CommentNode, depth = 0): ReactNode {
    return (
      <CommentItem
        key={node.comment.id}
        comment={node.comment}
        actions={actions}
        currentUserId={currentUserId}
        readOnly={readOnly}
        depth={depth}
      >
        {node.children.length > 0 ? (
          <ul className="comment-thread__list">
            {node.children.map((child) => renderNode(child, depth + 1))}
          </ul>
        ) : null}
      </CommentItem>
    );
  }

  if (list.isLoading) return <QueryLoading />;
  if (list.error) {
    const notFound = list.error instanceof ProblemError && list.error.status === 404;
    if (notFound) return null;
    return (
      <QueryError
        message={loadErrorMessage(list.error)}
        onRetry={() => {
          void list.refetch();
        }}
      />
    );
  }

  return (
    <section
      id={testId}
      className="comment-panel"
      aria-label={t("comment.title")}
      data-testid={testId}
    >
      <h2 className="comment-panel__title">{t("comment.title")}</h2>
      {actions.actionError ? (
        <p role="alert" className="comment-panel__error">
          {actions.actionError}
        </p>
      ) : null}
      {roots.length === 0 ? <EmptyState title={t("comment.empty")} /> : null}
      {roots.length > 0 ? (
        <ul className="comment-thread__list">{roots.map((root) => renderNode(root))}</ul>
      ) : null}
      {list.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          disabled={list.isFetchingNextPage}
          onClick={() => {
            void list.fetchNextPage();
          }}
        >
          {t("comment.list.loadMore")}
        </Button>
      ) : null}
      {!readOnly ? <CommentCompose actions={actions} /> : null}
    </section>
  );
}
