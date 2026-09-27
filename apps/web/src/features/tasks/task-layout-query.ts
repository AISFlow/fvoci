import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { encodeViewQueryParam, type ViewQuery } from "@/lib/view-query";
import { shiftMonth } from "./month-view-query";

export type GanttLayoutOutput = components["schemas"]["GanttLayoutOutput"];

interface GanttFilters {
  year: number;
  month: number;
  weekStartsOn: 0 | 1;
  pack: "rows" | "overlap";
  laneHeight: number;
  query: ViewQuery;
}

export function ganttLayoutQueryOptions(
  workspaceId: string | null,
  projectId: string | null,
  filters: GanttFilters,
) {
  return queryOptions({
    queryKey: ["task-layout", workspaceId, projectId, filters] as const,
    queryFn: async ({ signal }) => {
      const params = new URLSearchParams({
        year: String(filters.year),
        month: String(filters.month),
        weekStartsOn: String(filters.weekStartsOn),
        pack: filters.pack,
        laneHeight: String(filters.laneHeight),
      });
      const encoded = encodeViewQueryParam(filters.query);
      if (encoded) params.set("query", encoded);
      const result = await api.GET(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout",
        {
          params: {
            path: {
              workspace_id: workspaceId ?? "",
              project_id: projectId ?? "",
            },
            query: {
              year: filters.year,
              month: filters.month,
              weekStartsOn: filters.weekStartsOn,
              pack: filters.pack,
              laneHeight: filters.laneHeight,
              ...(encoded ? { query: encoded } : {}),
            },
          },
          signal,
        },
      );
      return ensureOk(result);
    },
    enabled: workspaceId !== null && projectId !== null,
  });
}

export function prefetchAdjacentLayouts(
  queryClient: import("@tanstack/react-query").QueryClient,
  workspaceId: string,
  projectId: string,
  filters: GanttFilters,
): void {
  for (const delta of [-1, 1] as const) {
    void queryClient.prefetchQuery(
      ganttLayoutQueryOptions(workspaceId, projectId, {
        ...filters,
        ...shiftMonth(filters.year, filters.month, delta),
      }),
    );
  }
}
