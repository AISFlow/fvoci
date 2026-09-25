import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import type { components } from "@/generated/api";
import { meQuery } from "@/lib/queries";
import { taskActivityQuery, type TaskActivityFilter } from "@/features/tasks/queries";
import { CommentCompose, CommentItem, useCommentActions } from "./comment-actions";
import {
  FALLBACK_TIME_ZONE,
  TaskActivityChangeItem,
  formatActivityTime,
  type ActivityChangeItem,
} from "./task-activity-item";
import "./comments.css";

type ApiActivityItem = components["schemas"]["ActivityItemOutput"];
type ActivityCommentItem = Extract<ApiActivityItem, { type: "comment" }>;

function changeItem(item: Extract<ApiActivityItem, { type: "change" }>): ActivityChangeItem {
  return {
    ...item,
    actor: item.actor ?? null,
    kind: item.kind === "created" ? "created" : "changed",
    changes: item.changes as ActivityChangeItem["changes"],
  };
}

/** Task comments and field changes in one feed (source comment-thread activity mode). */
export function TaskActivityPanel({
  workspaceId,
  taskId,
  currentUserId,
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
  const me = useQuery(meQuery);
  const timeZone = me.data?.timezone || FALLBACK_TIME_ZONE;
  const actions = useCommentActions({
    workspaceId,
    kind: "task",
    targetId: taskId,
    invalidate: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: ["task-activity", workspaceId, taskId] }),
        queryClient.invalidateQueries({ queryKey: ["comments", workspaceId, "task", taskId] }),
      ]),
  });

  const activityItems = useMemo(
    () => activity.data?.pages.flatMap((page) => page.items) ?? [],
    [activity.data],
  );

  function renderComment(item: ActivityCommentItem) {
    return (
      <CommentItem
        key={`comment:${item.id}`}
        comment={item.comment}
        actions={actions}
        currentUserId={currentUserId}
        readOnly={readOnly}
        before={
          item.parent ? (
            <div className="comment-thread__parent-preview">
              <p className="comment-thread__parent-label">
                {t("task.activity.replyTo", {
                  name: item.parent.actor?.name ?? t("task.activity.actor.unknown"),
                })}
              </p>
              <p>{item.parent.body}</p>
            </div>
          ) : null
        }
        meta={
          <p className="comment-thread__meta">
            {item.actor?.name ?? t("task.activity.actor.unknown")} ·{" "}
            <time dateTime={item.createdAt}>{formatActivityTime(item.createdAt, timeZone)}</time>
          </p>
        }
      />
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
      data-testid="task-comments"
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
      {actions.actionError ? (
        <p role="alert" className="comment-panel__error">
          {actions.actionError}
        </p>
      ) : null}
      {activityItems.length === 0 ? <EmptyState title={t("task.activity.empty")} /> : null}
      <ul className="task-activity-panel__list">
        {activityItems.map((item) =>
          item.type === "change" ? (
            <TaskActivityChangeItem
              key={`change:${item.id}`}
              item={changeItem(item)}
              timeZone={timeZone}
            />
          ) : (
            renderComment(item)
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
      {!readOnly && filter !== "changes" ? <CommentCompose actions={actions} /> : null}
    </section>
  );
}
