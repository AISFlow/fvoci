// A project collection's table, board and calendar views: the view types, a
// view's default config and the config a saved view holds. Framework-neutral,
// for the React and Vue collection pages.
import type { CollectionConfig, CollectionView } from "@/lib/queries/collections";
import { normalizeViewQuery, type ViewQuery } from "@/lib/view-query";

export type CollectionViewType = "table" | "board" | "calendar";

/** Rows per table (or ungrouped board, or calendar day list) page. */
export const PAGE_LIMIT = 50;

export function defaultConfig(type: CollectionViewType, query?: ViewQuery): CollectionConfig {
  return {
    query: query ?? { filters: {}, sort: [] },
    groupBy: type === "board" ? "status" : null,
    dateBy: type === "calendar" ? "due" : null,
  };
}

/** Saved `config` → typed config (unknown shapes fall back to the type default). */
export function collectionConfigOf(view: CollectionView): CollectionConfig {
  const raw = view.config as unknown as Record<string, unknown>;
  const query = normalizeViewQuery(raw?.query) ?? { filters: {}, sort: [] };
  const type = view.type === "board" || view.type === "calendar" ? view.type : "table";
  const base = defaultConfig(type, query);
  return {
    query,
    groupBy:
      typeof raw?.groupBy === "string" ? raw.groupBy : raw?.groupBy === null ? null : base.groupBy,
    dateBy:
      typeof raw?.dateBy === "string" ? raw.dateBy : raw?.dateBy === null ? null : base.dateBy,
  };
}

export function isViewType(value: string): value is CollectionViewType {
  return value === "table" || value === "board" || value === "calendar";
}

/** Short Korean weekday names from the user's first day of the week (0 Sunday, 1 Monday). */
export function weekdayNames(weekStartsOn: number): string[] {
  const formatter = new Intl.DateTimeFormat("ko-KR", { weekday: "short", timeZone: "UTC" });
  // 2026-09-06 is a Sunday.
  return Array.from({ length: 7 }, (_, index) =>
    formatter.format(new Date(Date.UTC(2026, 8, 6 + ((weekStartsOn + index) % 7)))),
  );
}
