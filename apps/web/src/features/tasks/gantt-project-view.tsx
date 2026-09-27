import { formatDisplayId } from "@/lib/href";
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { loadErrorMessage, QueryError } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { QueryLoading } from "@/components/query-status";
import { ProjectViewNav } from "@/features/collections/project-view-nav";
import { projectQuery, workflowQuery } from "@/features/projects/queries";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { itemPath, projectGanttPath, projectPath, projectsPath } from "@/lib/href";
import { membersQuery } from "@/lib/queries";
import { EMPTY_VIEW_QUERY, withTitleFilter, type ViewQuery } from "@/lib/view-query";
import { ganttBarPatch } from "./gantt-bar";
import { GanttChart } from "./gantt/GanttChart";
import "./gantt/theme.css";
import "./gantt-project-view.css";
import { shiftMonth } from "./month-view-query";
import { ganttLayoutQueryOptions } from "./task-layout-query";
import type { IsoDate } from "./gantt/types";
import { formatPersonName } from "@fvoci/i18n";

function utcYearMonth(): { year: number; month: number } {
  const d = new Date();
  return { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1 };
}

export function GanttProjectView({
  workspaceId,
  slug,
  projectId,
  projectKey,
  year: urlYear,
  month: urlMonth,
  query,
}: {
  workspaceId: string;
  slug: string;
  projectId: string;
  projectKey: string;
  year?: number;
  month?: number;
  query?: ViewQuery;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const initial = utcYearMonth();
  const year = urlYear ?? initial.year;
  const month = urlMonth ?? initial.month;
  const goMonth = (delta: -1 | 1) => {
    const next = shiftMonth(year, month, delta);
    const base = projectGanttPath(slug, projectKey);
    const params = new URLSearchParams(window.location.search);
    params.set("y", String(next.year));
    params.set("m", String(next.month));
    void navigate(`${base}?${params.toString()}`, { replace: true });
  };
  const [flow, setFlow] = useState(false);
  const [barError, setBarError] = useState<string | null>(null);
  const projectQ = useQuery(projectQuery(workspaceId, projectId));
  const workflowQ = useQuery(workflowQuery(workspaceId, projectId));
  const membersQ = useQuery(membersQuery(workspaceId));
  const viewQuery = query ?? EMPTY_VIEW_QUERY;
  const layoutFilters = {
    year,
    month,
    weekStartsOn: 0 as 0 | 1,
    pack: flow ? ("overlap" as const) : ("rows" as const),
    laneHeight: 48,
    query: viewQuery,
  };
  const listOptions = ganttLayoutQueryOptions(workspaceId, projectId, layoutFilters);
  const listQuery = useQuery(listOptions);
  const readOnly = projectQ.data?.status === "archived";
  const tasks = listQuery.data?.items ?? [];
  const layout = listQuery.data;
  const byId = useMemo(() => new Map(tasks.map((item) => [item.id, item])), [tasks]);
  const statusById = useMemo(
    () => new Map((workflowQ.data?.statuses ?? []).map((s) => [s.id, s])),
    [workflowQ.data?.statuses],
  );
  const memberById = useMemo(() => {
    const rows = membersQ.data?.items ?? [];
    return new Map(rows.map((m) => [m.userId, formatPersonName(m)]));
  }, [membersQ.data]);

  const barMutation = useMutation({
    mutationFn: async (p: { id: string; start: IsoDate; end: IsoDate }) => {
      const task = tasks.find((item) => item.id === p.id);
      if (!task) throw new Error("task not found");
      const body = ganttBarPatch(
        {
          startDate: task.startDate ?? null,
          dueDate: task.dueDate ?? null,
          dueAt: task.dueAt ?? null,
        },
        p,
      );
      const result = await api.PATCH("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
        params: { path: { workspace_id: workspaceId, task_id: p.id } },
        body,
      });
      return ensureOk(result);
    },
    onSuccess: () => {
      setBarError(null);
      void queryClient.invalidateQueries({ queryKey: listOptions.queryKey });
    },
    onError: (err) => {
      setBarError(err instanceof ProblemError ? err.title : t("gantt.bar.failed"));
    },
  });

  if (projectQ.error) {
    return (
      <p role="alert" className="break-keep text-ui text-destructive">
        {t("project.load.failed")}
      </p>
    );
  }

  const loading = listQuery.isPending || projectQ.isLoading;

  return (
    <div className="fvoci-gantt-shell flex flex-col gap-3">
      <p className="task-home__crumb">
        <Link to={projectsPath(slug)}>{t("nav.projects")}</Link>
        <span aria-hidden="true"> / </span>
        <Link to={projectPath(slug, projectKey)}>{projectKey}</Link>
      </p>
      <ProjectViewNav slug={slug} projectKey={projectKey} active="gantt" />
      <div className="fvoci-gantt-shell__toolbar" data-slot="gantt-toolbar">
        <div className="fvoci-gantt-shell__nav">
          <Button type="button" variant="outline" size="sm" onClick={() => goMonth(-1)}>
            {t("cal.prevMonth")}
          </Button>
          <p className="fvoci-gantt-shell__period break-keep text-ui">
            {t("cal.yearMonth", { year, month })}
          </p>
          <Button type="button" variant="outline" size="sm" onClick={() => goMonth(1)}>
            {t("cal.nextMonth")}
          </Button>
        </div>
        <Button
          type="button"
          variant={flow ? "default" : "outline"}
          size="sm"
          aria-pressed={flow}
          onClick={() => setFlow((v) => !v)}
        >
          {t("gantt.flow")}
        </Button>
        <div className="fvoci-gantt-shell__search">
          <Input
            type="search"
            className="h-11 w-full min-w-0 sm:w-56"
            aria-label={t("gantt.search")}
            placeholder={t("gantt.search")}
            value={viewQuery.filters.title ?? ""}
            maxLength={200}
            onChange={(event) => {
              const next = withTitleFilter(viewQuery, event.target.value);
              const params = new URLSearchParams(window.location.search);
              const encoded = JSON.stringify(next);
              if (next.filters.title) params.set("query", encoded);
              else params.delete("query");
              void navigate(`${projectGanttPath(slug, projectKey)}?${params.toString()}`, {
                replace: true,
              });
            }}
          />
        </div>
      </div>
      {barError ? (
        <p role="alert" className="break-keep text-ui text-destructive">{barError}</p>
      ) : null}
      {layout?.truncated ? (
        <p role="status" className="break-keep text-ui text-muted-foreground">
          {t("gantt.truncated")}
        </p>
      ) : null}
      {loading ? (
        <div className="fvoci-gantt-shell__state">
          <QueryLoading />
        </div>
      ) : listQuery.isError ? (
        <QueryError message={loadErrorMessage(listQuery.error)} onRetry={() => void listQuery.refetch()} />
      ) : !layout || tasks.length === 0 ? (
        <EmptyState title={t("task.view.empty")} />
      ) : (
        <GanttChart
          className="fvoci-gantt-shell__chart"
          showRail
          layout={layout}
          renderRailRow={(taskId) => {
            const item = byId.get(taskId);
            if (!item) return null;
            const status = statusById.get(item.statusId);
            const assigneeName = item.assigneeIds
              .map((id) => memberById.get(id))
              .find((n): n is string => n !== undefined);
            return (
              <span className="flex min-w-0 items-center gap-1">
                {status ? (
                  <span className="shrink-0 text-caption text-muted-foreground" title={status.name}>
                    {status.name}
                  </span>
                ) : null}
                <span className="shrink-0 font-mono text-caption text-muted-foreground">
                  {formatDisplayId(projectKey, item.number)}
                </span>
                <span className="min-w-0 flex-1 truncate">{item.title}</span>
                {assigneeName ? (
                  <span className="max-w-16 shrink-0 truncate text-dense text-muted-foreground">
                    {assigneeName}
                  </span>
                ) : null}
              </span>
            );
          }}
          onSelect={(taskId) => {
            const item = byId.get(taskId);
            if (!item) return;
            void navigate(itemPath(slug, formatDisplayId(projectKey, item.number)));
          }}
          onBarChange={
            readOnly || barMutation.isPending
              ? undefined
              : (p) => barMutation.mutate(p)
          }
        />
      )}
    </div>
  );
}
