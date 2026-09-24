import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { Button } from "@/components/ui/button";
import { formatDisplayId, itemPath } from "@/lib/href";
import type { WorkflowStatus } from "@/features/projects/queries";
import { visibleTaskStatusSections, type TaskListStatusCount } from "./task-list-page";
import { TaskListLoadMore } from "./task-list-load-more";
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
  const sections = visibleTaskStatusSections(
    items,
    statuses,
    statusCounts,
    t("task.list.otherStatus"),
  );
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
        {sections.map((section) => {
          const open = !collapsed.has(section.id);
          return (
            <section key={section.id} className="task-status">
              <div className="task-status__head">
                <button
                  type="button"
                  className="task-status__toggle"
                  aria-expanded={open}
                  onClick={() =>
                    setCollapsed((cur) => {
                      const next = new Set(cur);
                      if (next.has(section.id)) next.delete(section.id);
                      else next.add(section.id);
                      return next;
                    })
                  }
                >
                  <span>{section.name}</span>
                  <span className="task-status__count">{section.count}</span>
                </button>
                {canCreate && section.known ? (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    aria-label={t("task.list.createInStatus", { status: section.name })}
                    onClick={() => onCreateClick(section.id)}
                  >
                    {t("task.create")}
                  </Button>
                ) : null}
              </div>
              {open ? (
                <ul className="task-status-list">
                  {section.items.map((item) => {
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
        <TaskListLoadMore
          error={loadMoreError}
          pending={loadMorePending}
          onLoadMore={onLoadMore}
        />
      ) : null}
    </div>
  );
}
