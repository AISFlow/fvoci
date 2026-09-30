import type { SearchTab } from "@/lib/queries";

export const SEARCH_TABS: readonly SearchTab[] = [
  "all",
  "document",
  "task",
  "attachment",
  "comment",
];

const TAB_SET = new Set<string>(SEARCH_TABS);

/** Unknown or missing `tab` query values fall back to `all`, as in SearchPage.tsx. */
export function parseSearchTab(raw: unknown): SearchTab {
  return typeof raw === "string" && TAB_SET.has(raw) ? (raw as SearchTab) : "all";
}
