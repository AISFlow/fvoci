import { canonicalizeProjectKey, parseItemRef } from "@/lib/href";

export type ParentSearchMode =
  | { kind: "list"; title?: string }
  | { kind: "display-id"; displayId: string }
  | { kind: "empty" };

/** Source `listTaskParents`: display IDs are lookup, not a title ILIKE scan. */
export function parentSearchMode(q: string, projectKey: string): ParentSearchMode {
  const trimmed = q.trim();
  const parsed = parseItemRef(trimmed);
  if (parsed) {
    if (parsed.prefix !== canonicalizeProjectKey(projectKey)) return { kind: "empty" };
    return { kind: "display-id", displayId: parsed.displayId };
  }
  return { kind: "list", title: trimmed === "" ? undefined : trimmed };
}

/** Source parent candidates: non-subtask children attach only to epics. */
export function parentTypeFilter(childType: string): string | undefined {
  if (childType === "epic" || childType === "subtask") return undefined;
  return "epic";
}

export function parentListViewQuery(childType: string, title?: string): string {
  const filters: { type?: string; title?: string } = {};
  const type = parentTypeFilter(childType);
  if (type) filters.type = type;
  if (title) filters.title = title;
  return JSON.stringify({ filters });
}
