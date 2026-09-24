import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";
import { formatDisplayId, itemPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import { statusCountFor, type TaskListStatusCount } from "./task-list-page";
import { taskTypeLabel } from "./task-types";
import type { TaskListItem } from "./queries";
import "@/features/projects/projects.css";

export function TaskList({
  slug,
  projectKey,
  items,
  statusCounts,
  statuses,
  canCreate,
  defaultStatusId,
  hasMore,
  loadMorePending,
  loadMoreError,
  onCreateClick,
  onLoadMore,
}: {
  slug: string;
  projectKey: string;
  items: readonly TaskListItem[];
  statusCounts: readonly TaskListStatusCount[];
  statuses: readonly WorkflowStatus[];
  canCreate: boolean;
  defaultStatusId: string | null;
  hasMore: boolean;
  loadMorePending?: boolean;
  loadMoreError?: string | null;
  onCreateClick: (statusId: string) => void;
  onLoadMore: () => void;
}) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const byStatus = new Map<string, TaskListItem[]>();
  for (const status of statuses) byStatus.set(status.id, []);
  for (const item of items) {
    const group = byStatus.get(item.statusId);
    if (group) group.push(item);
    else byStatus.set(item.statusId, [item]);
  }
  const catalogEmpty = items.length === 0 && !statusCounts.some((row) => row.count > 0);

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center justify-end gap-2">
        {canCreate && defaultStatusId ? (
          <Button type="button" size="sm" onClick={() => onCreateClick(defaultStatusId)}>
            {t("task.create.new")}
          </Button>
        ) : null}
      </div>
      {catalogEmpty ? <EmptyState title={t("task.view.empty")} /> : null}
      <div className="flex flex-col gap-8">
        {statuses.map((status) => {
          const group = byStatus.get(status.id) ?? [];
          const count = statusCountFor(statusCounts, status.id) ?? 0;
          if (count === 0) return null;
          const open = !collapsed.has(status.id);
          return (
            <section key={status.id} className="task-status">
              <div className="task-status__head">
                <button
                  type="button"
                  className="task-status__toggle"
                  aria-expanded={open}
                  onClick={() =>
                    setCollapsed((cur) => {
                      const next = new Set(cur);
                      if (next.has(status.id)) next.delete(status.id);
                      else next.add(status.id);
                      return next;
                    })
                  }
                >
                  <span>{status.name}</span>
                  <span className="task-status__count">{count}</span>
                </button>
                {canCreate ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    aria-label={t("task.list.createInStatus", { status: status.name })}
                    onClick={() => onCreateClick(status.id)}
                  >
                    {t("task.create")}
                  </Button>
                ) : null}
              </div>
              {open ? (
                <ul className="task-status-list">
                  {group.map((item) => {
                    const displayId = formatDisplayId(projectKey, item.number);
                    return (
                      <li key={item.id}>
                        <Link
                          to={itemPath(slug, displayId)}
                          className="task-row"
                          data-testid={`task-row-${item.id}`}
                        >
                          <span className="task-row__id">{displayId}</span>
                          <span className="task-row__title">{item.title}</span>
                          <span className="project-list__private">{taskTypeLabel(item.type)}</span>
                        </Link>
                      </li>
                    );
                  })}
                </ul>
              ) : null}
            </section>
          );
        })}
      </div>
      {hasMore ? (
        <div className="flex flex-col items-start gap-2">
          {loadMoreError ? (
            <p role="alert" className="task-form__alert">
              {loadMoreError}
            </p>
          ) : null}
          <Button type="button" variant="outline" disabled={loadMorePending} onClick={onLoadMore}>
            {loadMorePending ? t("load.loading") : t("task.list.loadMore")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
