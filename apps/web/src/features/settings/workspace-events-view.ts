// Adapted from source apps/web/src/features/settings/settings-activity.tsx
// (ActivityFeedCard): verb and time per event, plus cursor "load more".
import { createElement } from "react";
import { t } from "@fvoci/i18n";
import type { components } from "@/generated/api";
import { formatInstant } from "@/lib/datetime";
import { TaskListLoadMore } from "@/features/tasks/task-list-load-more";

export type WorkspaceEvent = components["schemas"]["WorkspaceEventOutput"];

const cellClass = "border-b border-border px-2 py-2 align-top text-ui";

export function WorkspaceEventsView({
  items,
  timeZone,
  loading,
  error,
  onRetry,
  hasMore,
  loadingMore,
  loadMoreError,
  onLoadMore,
}: {
  items: WorkspaceEvent[];
  timeZone: string;
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  hasMore: boolean;
  loadingMore: boolean;
  loadMoreError: string | null;
  onLoadMore: () => void;
}) {
  return createElement(
    "section",
    { className: "settings-section", "aria-labelledby": "workspace-events-title" },
    createElement(
      "h2",
      { className: "settings-section__title text-title", id: "workspace-events-title" },
      t("settings.activity"),
    ),
    createElement(
      "div",
      { className: "flex flex-col gap-2", "data-testid": "workspace-events" },
      loading
        ? createElement(
            "p",
            { role: "status", className: "text-ui text-muted-foreground" },
            t("load.loading"),
          )
        : null,
      error
        ? createElement(
            "div",
            { className: "flex flex-col items-start gap-2" },
            createElement("p", { role: "alert", className: "text-ui text-destructive" }, error),
            createElement(
              "button",
              {
                type: "button",
                className: "text-ui underline underline-offset-2",
                onClick: onRetry,
              },
              t("load.retry"),
            ),
          )
        : null,
      !loading && !error && items.length === 0
        ? createElement("p", { className: "text-ui text-muted-foreground" }, t("audit.empty"))
        : null,
      items.length > 0
        ? createElement(
            "div",
            { className: "overflow-x-auto" },
            createElement(
              "table",
              { className: "w-full border-collapse" },
              createElement(
                "tbody",
                null,
                items.map((row) =>
                  createElement(
                    "tr",
                    { key: row.id },
                    createElement("td", { className: `${cellClass} font-mono` }, row.verb),
                    createElement(
                      "td",
                      { className: `${cellClass} settings-tabular` },
                      formatInstant(row.createdAt, timeZone, {
                        year: "numeric",
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                      }),
                    ),
                  ),
                ),
              ),
            ),
          )
        : null,
      hasMore && !error
        ? createElement(TaskListLoadMore, {
            error: loadMoreError,
            pending: loadingMore,
            onLoadMore,
          })
        : null,
    ),
  );
}
