import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

export type LookupItem = components["schemas"]["LookupItemOutput"];
export type LookupList = components["schemas"]["LookupListResponse"];

export type LookupTarget =
  | { kind: "task"; item: LookupItem }
  | { kind: "project-document"; item: LookupItem }
  | { kind: "miss" };

export function resolveLookupTarget(
  items: readonly LookupItem[],
  displayId: string,
): LookupTarget {
  const want = displayId.trim().toUpperCase();
  if (want === "") return { kind: "miss" };
  const matches = items.filter((item) => item.displayId.toUpperCase() === want);
  const taskItem = matches.find((item) => item.kind === "task");
  if (taskItem) return { kind: "task", item: taskItem };
  const projectDoc = matches.find(
    (item) => item.kind === "document" && item.projectId != null && item.projectId !== "",
  );
  if (projectDoc) return { kind: "project-document", item: projectDoc };
  return { kind: "miss" };
}

export function pickLookupTask(
  items: readonly LookupItem[],
  displayId: string,
): LookupItem | null {
  const target = resolveLookupTarget(items, displayId);
  return target.kind === "task" ? target.item : null;
}

export function lookupQuery(workspaceId: string, displayId: string) {
  return queryOptions({
    queryKey: ["lookup", workspaceId, displayId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/lookup/{display_id}", {
          params: {
            path: { workspace_id: workspaceId, display_id: displayId },
          },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(displayId),
    retry: false,
  });
}
