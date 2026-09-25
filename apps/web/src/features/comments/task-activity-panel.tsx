import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import type { components } from "@/generated/api";
import { ensureOk, api } from "@/lib/api";
import { taskActivityQuery, type TaskActivityFilter } from "@/features/tasks/queries";
import { nextReplyTarget } from "./comment-drafts";
import type { CommentNode, CommentOutput } from "./comment-tree";
import {
  TaskActivityChangeItem,
  type ActivityChangeItem,
} from "./task-activity-item";
import "./comments.css";

type ApiActivityItem = components["schemas"]["ActivityItemOutput"];

type ActivityCommentItem = {
  type: "comment";
  id: string;
  createdAt: string;
  actor: { id: string; name: string } | null;
  comment: CommentOutput;
  parent: components["schemas"]["ActivityCommentParentOutput"] | null;
};

type ActivityItem = ActivityChangeItem | ActivityCommentItem;

function normalizeActivityItem(item: ApiActivityItem): ActivityItem {
  if (item.type === "change") {
    return {
      type: "change",
      id: item.id,
      createdAt: item.created_at,
      actor: item.actor ?? null,
      channel: item.channel,
      kind: item.kind === "created" ? "created" : "changed",
      changes: item.changes as ActivityChangeItem["changes"],
    };
  }
  return {
    type: "comment",
    id: item.id,
    createdAt: item.created_at,
    actor: item.actor ?? null,
    comment: item.comment,
    parent: item.parent ?? null,
  };
}

export function TaskActivityPanel({
  workspaceId,
  taskId,
  currentUserId: _currentUserId,
  readOnly = false,
}: {
  workspaceId: string;
  taskId: string;
  currentUserId: string;
  readOnly?: boolean;
}) {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<TaskActivityFilter>("all");
  const activity = useInfiniteQuery(taskActivityQuery(workspaceId, taskId, filter));
  const [draft, setDraft] = useState("");
  const [replyDraft, setReplyDraft] = useState("");
  const [replyToId, setReplyToId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const activityItems = useMemo(
    () =>
      activity.data?.pages.flatMap((page) => page.items.map(normalizeActivityItem)) ?? [],
    [activity.data],
  );

  const invalidate = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["task-activity", workspaceId, taskId] }),
      queryClient.invalidateQueries({ queryKey: ["comments", workspaceId, "task", taskId] }),
    ]);
  };

  const create = useMutation({
    mutationFn: async (body: { text: string; parentId?: string | null }) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
          body: {
            body: body.text,
            parentId: body.parentId ?? undefined,
          },
        }),
      ),
    onSuccess: async () => {
      setDraft("");
      setReplyDraft("");
      setReplyToId(null);
      setActionError(null);
      await invalidate();
    },
    onError: (error: unknown) => setActionError(loadErrorMessage(error)),
  });

  const pending = create.isPending;

  function renderComment(node: CommentNode, activity?: ActivityCommentItem) {
    const { comment } = node;
    const author = activity?.actor?.name ?? comment.createdBy;
    return (
      <li key={activity ? `comment:${activity.id}` : comment.id} className="comment-thread__item">
        {activity?.parent ? (
          <div className="comment-thread__parent-preview">
            <p className="comment-thread__parent-label">
              {t("task.activity.replyTo", {
                name: activity.parent.actor?.name ?? t("task.activity.actor.unknown"),
              })}
            </p>
            <p>{activity.parent.body}</p>
          </div>
        ) : null}
        <p className="comment-thread__body">{comment.body}</p>
        <p className="comment-thread__meta">{author}</p>
        {!readOnly ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={pending}
            onClick={() => setReplyToId((current) => nextReplyTarget(current, comment.id))}
          >
            {t("comment.reply")}
          </Button>
        ) : null}
        {!readOnly && replyToId === comment.id ? (
          <form
            className="comment-thread__compose"
            onSubmit={(event) => {
              event.preventDefault();
              void create.mutateAsync({ text: replyDraft, parentId: comment.id });
            }}
          >
            <textarea
              aria-label={t("comment.placeholder")}
              value={replyDraft}
              onChange={(event) => setReplyDraft(event.target.value)}
            />
            <Button type="submit" disabled={pending || replyDraft.trim().length === 0}>
              {t("comment.submit")}
            </Button>
          </form>
        ) : null}
      </li>
    );
  }

  if (activity.isLoading) return <QueryLoading />;
  if (activity.error) {
    return (
      <QueryError
        message={loadErrorMessage(activity.error)}
        onRetry={() => {
          void activity.refetch();
        }}
      />
    );
  }

  return (
    <section
      id="fv-comments"
      tabIndex={-1}
      className="comment-panel task-activity-panel"
      aria-label={t("task.activity.title")}
    >
      <div className="comment-panel__header">
        <h2>{t("task.activity.title")}</h2>
        <label className="task-activity-panel__filter">
          <span className="sr-only">{t("task.activity.filter.label")}</span>
          <select
            aria-label={t("task.activity.filter.label")}
            value={filter}
            onChange={(event) => setFilter(event.target.value as TaskActivityFilter)}
          >
            <option value="all">{t("task.activity.filter.all")}</option>
            <option value="comments">{t("task.activity.filter.comments")}</option>
            <option value="changes">{t("task.activity.filter.changes")}</option>
          </select>
        </label>
      </div>
      {actionError ? <p role="alert">{actionError}</p> : null}
      {activityItems.length === 0 ? (
        <EmptyState title={t("task.activity.empty")} />
      ) : null}
      <ul className="task-activity-panel__list">
        {activityItems.map((item) =>
          item.type === "change" ? (
            <TaskActivityChangeItem key={`change:${item.id}`} item={item} />
          ) : (
            renderComment({ comment: item.comment, children: [] }, item)
          ),
        )}
      </ul>
      {activity.hasNextPage ? (
        <Button
          type="button"
          variant="outline"
          disabled={activity.isFetchingNextPage}
          onClick={() => void activity.fetchNextPage()}
        >
          {t("task.activity.loadMore")}
        </Button>
      ) : null}
      {!readOnly && filter !== "changes" ? (
        <div data-comment-compose="">
          <form
            className="comment-thread__compose"
            onSubmit={(event) => {
              event.preventDefault();
              void create.mutateAsync({ text: draft, parentId: null });
            }}
          >
            <textarea value={draft} onChange={(event) => setDraft(event.target.value)} />
            <Button type="submit" disabled={pending || draft.trim().length === 0}>
              {t("comment.submit")}
            </Button>
          </form>
        </div>
      ) : null}
    </section>
  );
}
