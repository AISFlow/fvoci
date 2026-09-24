import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

export type LookupItem = components["schemas"]["LookupItemOutput"];
export type LookupList = components["schemas"]["LookupListResponse"];

export function pickLookupTask(
  items: readonly LookupItem[],
  displayId: string,
): LookupItem | null {
  const want = displayId.trim().toUpperCase();
  if (want === "") return null;
  for (const item of items) {
    if (item.kind === "task" && item.displayId.toUpperCase() === want) return item;
  }
  return null;
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
