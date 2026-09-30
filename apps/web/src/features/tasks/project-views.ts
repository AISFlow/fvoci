// Saved task views of a project (the `/projects/{id}/views` list): their
// types and the view query a saved config holds. Framework-neutral, for the
// React and Vue task list pages.
import { t } from "@fvoci/i18n";
import type { ProjectView } from "@/lib/queries/collections";
import { normalizeViewQuery, type ViewQuery } from "@/lib/view-query";

export const PROJECT_VIEW_TYPES = ["list", "board", "calendar", "gantt", "table"] as const;
export type ProjectViewType = (typeof PROJECT_VIEW_TYPES)[number];

export function projectViewTypeLabel(type: string): string {
  switch (type) {
    case "list":
      return t("view.backlog");
    case "board":
      return t("view.board");
    case "calendar":
      return t("view.calendar");
    case "gantt":
      return t("view.gantt");
    case "table":
      return t("view.table");
    default:
      return type;
  }
}

export function viewConfigOf(view: ProjectView): ViewQuery {
  return normalizeViewQuery(view.config) ?? { filters: {}, sort: [] };
}
