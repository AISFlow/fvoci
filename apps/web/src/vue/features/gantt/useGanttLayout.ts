import { keepPreviousData } from "@tanstack/query-core";
import { useQuery } from "@tanstack/vue-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";
import { encodeViewQueryParam, type ViewQuery } from "@/lib/view-query";

export type GanttLayout = components["schemas"]["GanttLayoutOutput"];
export type GanttLayoutItem = components["schemas"]["GanttLayoutItemOutput"];

export interface GanttLayoutFilters {
  readonly year: number;
  readonly month: number;
  readonly weekStartsOn: 0 | 1;
  readonly query: ViewQuery;
}

/**
 * The project's month of tasks, links, calendar and edit permission in one
 * snapshot. Keyed under ["task-layout", ws, project], which the task stream
 * and every task write invalidate (features/tasks/task-cache.ts). The chart
 * lays itself out from items/links/calendar; the server's pixel fields are
 * not requested.
 */
export function ganttLayoutQuery(workspaceId: string, projectId: string, filters: GanttLayoutFilters) {
  const encoded = encodeViewQueryParam(filters.query);
  return queryOptions({
    queryKey: [
      "task-layout",
      workspaceId,
      projectId,
      { year: filters.year, month: filters.month, weekStartsOn: filters.weekStartsOn, query: encoded ?? null },
    ] as const,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout", {
          params: {
            path: { workspace_id: workspaceId, project_id: projectId },
            query: {
              year: filters.year,
              month: filters.month,
              weekStartsOn: filters.weekStartsOn,
              ...(encoded ? { query: encoded } : {}),
            },
          },
          signal,
        }),
      ),
    retry: false,
  });
}

export function useGanttLayout(
  args: () => { workspaceId: string | undefined; projectId: string | undefined; filters: GanttLayoutFilters },
) {
  return useQuery(() => {
    const { workspaceId = "", projectId = "", filters } = args();
    return {
      ...ganttLayoutQuery(workspaceId, projectId, filters),
      enabled: workspaceId !== "" && projectId !== "",
      // Another month or filter keeps the current chart on screen while it loads.
      placeholderData: keepPreviousData,
    };
  });
}
