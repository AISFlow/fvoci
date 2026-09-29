// Adapted from source apps/web/src/features/collections/collection-panel.tsx
// (`CollectionContents`), collection-cards.tsx and collection-calendar.tsx for a
// project task collection. The grouped board lives in collection-board.tsx;
// calendar date moves (drag a preview or a day-list row onto a day or the
// unassigned button) follow calendar-model.ts.
import { formatPersonName, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState, type DragEvent } from "react";
import { Link } from "react-router-dom";
import { ConfirmActionButton } from "@/components/confirm-action";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  api,
  ensureOk,
  isInvalidCursor,
  isInvalidInput,
  ProblemError,
  problemMessage,
} from "@/lib/api";
import {
  asCollectionValue,
  formatCollectionValue,
  isMonth,
  monthGrid,
  monthWindow,
  shiftMonth,
  SORTABLE_FIELD_TYPES,
  todayInTimeZone,
  type CollectionValue,
} from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import { FALLBACK_TZ } from "@/lib/datetime";
import { itemPath } from "@/lib/href";
import { membersQuery, meQuery } from "@/lib/queries";
import {
  asJsonObject,
  collectionFieldsQuery,
  collectionPrefix,
  collectionRowsQuery,
  collectionViewsQuery,
  putCollectionValue,
  type CollectionConfig,
  type CollectionField,
  type CollectionQueryBody,
  type CollectionQueryItem,
  type CollectionQueryPreview,
  type CollectionView,
} from "@/lib/queries/collections";
import { readPrimarySort, setPrimarySort } from "@/lib/view-query";
import { moveRequest, type BoardGroup } from "./board-model";
import {
  CALENDAR_DRAG_TYPE,
  dateMovable,
  dateMoveRequest,
  type CalendarRow,
} from "./calendar-model";
import { CollectionBoard } from "./collection-board";
import {
  collectionConfigOf,
  defaultConfig,
  isViewType,
  PAGE_LIMIT,
  weekdayNames,
  type CollectionViewType,
} from "./collection-view";
import { CustomFilters } from "./custom-filters";
import { ValueEditor } from "./value-editor";
import "./collections.css";

export function CollectionContents({
  workspaceId,
  slug,
  collectionId,
  projectId,
  type,
  initialViewId,
  onOpenView,
}: {
  workspaceId: string;
  slug: string;
  collectionId: string;
  projectId: string;
  type: CollectionViewType;
  /** Saved view to load on mount (from the `?view=` search param). */
  initialViewId: string | null;
  /** Switch to a saved view, possibly of another type (route change). */
  onOpenView: (type: CollectionViewType, viewId: string | null) => void;
}) {
  const queryClient = useQueryClient();
  const baseId = useId();
  const prefix = collectionPrefix(workspaceId, collectionId);
  const fields = useQuery(collectionFieldsQuery(workspaceId, collectionId));
  const views = useQuery(collectionViewsQuery(workspaceId, collectionId));
  const members = useQuery(membersQuery(workspaceId));
  const me = useQuery(meQuery);
  const timeZone = me.data?.timezone ?? FALLBACK_TZ;
  const weekStartsOn = me.data?.weekStartsOn === 0 ? 0 : 1;

  const [config, setConfig] = useState<CollectionConfig>(() => defaultConfig(type));
  const [view, setView] = useState<CollectionView | null>(null);
  const [viewName, setViewName] = useState("");
  const [visibility, setVisibility] = useState<"private" | "shared">("private");
  const [viewConflict, setViewConflict] = useState(false);
  const [cursor, setCursor] = useState<string | undefined>();
  const [day, setDay] = useState<string | null | undefined>();
  const [month, setMonth] = useState<string>("");
  const [customOpen, setCustomOpen] = useState(false);
  const [moveError, setMoveError] = useState<string | null>(null);
  const [moving, setMoving] = useState(false);
  const draggedDate = useRef<CalendarRow | null>(null);
  /** Calendar drop target under the pointer: a day, null (unassigned) or none. */
  const [dropDay, setDropDay] = useState<string | null | undefined>(undefined);

  const effectiveMonth = isMonth(month) ? month : todayInTimeZone(timeZone).slice(0, 7);

  function applyView(saved: CollectionView | null) {
    setView(saved);
    setViewConflict(false);
    setCursor(undefined);
    setDay(undefined);
    if (saved) {
      setConfig(collectionConfigOf(saved));
      setViewName(saved.name);
      setVisibility(saved.visibility === "shared" ? "shared" : "private");
    } else {
      setConfig(defaultConfig(type));
      setViewName("");
      setVisibility("private");
    }
  }

  // The `?view=` search param is the source of truth for which saved view is open.
  useEffect(() => {
    if (!views.data || (initialViewId ?? null) === (view?.id ?? null)) return;
    if (!initialViewId) {
      applyView(null);
      return;
    }
    const saved = views.data.items.find((item) => item.id === initialViewId);
    if (saved) applyView(saved);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [views.data, initialViewId]);

  const calendarWindow =
    type === "calendar" && config.dateBy
      ? { ...monthWindow(effectiveMonth), timeZone }
      : undefined;
  // A grouped board pages each column itself; this request only brings the
  // group catalog and counts, so it asks for a single row.
  const groupedBoard = type === "board" && config.groupBy !== null;
  const body: CollectionQueryBody = {
    config,
    limit: groupedBoard ? 1 : PAGE_LIMIT,
    ...(cursor && !groupedBoard ? { cursor } : {}),
    ...(type === "calendar" && config.dateBy && day !== undefined ? { day } : {}),
    ...(calendarWindow ? { window: calendarWindow } : {}),
  };
  const rows = useQuery(
    collectionRowsQuery(
      workspaceId,
      collectionId,
      body,
      fields.isSuccess && views.isSuccess && me.isSuccess,
    ),
  );

  useEffect(() => {
    if (cursor && isInvalidCursor(rows.error)) {
      setCursor(undefined);
    }
  }, [cursor, rows.error]);

  async function refresh() {
    await queryClient.invalidateQueries({ queryKey: prefix });
  }

  function change(patch: Partial<CollectionConfig>) {
    setConfig((current) => ({ ...current, ...patch }));
    setCursor(undefined);
    setDay(undefined);
  }

  async function setValue(row: CollectionQueryItem, field: CollectionField, value: CollectionValue) {
    try {
      await putCollectionValue(workspaceId, collectionId, row.id, {
        fieldId: field.id,
        expectedVersion: row.version,
        expectedFieldVersion: field.version,
        value,
      });
    } finally {
      await refresh();
    }
  }

  // No optimistic update: the card moves only after the write succeeds and the
  // board refetches; a rejected move keeps its server error visible.
  async function moveToGroup(row: CollectionQueryItem, target: BoardGroup) {
    const request = moveRequest(config.groupBy, row, target);
    if (!request || moving) return;
    setMoveError(null);
    setMoving(true);
    try {
      if (request.kind === "status") {
        await ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move", {
            params: { path: { workspace_id: workspaceId, task_id: request.taskId } },
            body: { statusId: request.statusId, expectedStatusId: request.expectedStatusId },
          }),
        );
        await queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] });
      } else {
        const field = fields.data?.items.find((item) => item.id === request.fieldId);
        if (!field) return;
        await putCollectionValue(workspaceId, collectionId, row.id, {
          fieldId: field.id,
          expectedVersion: row.version,
          expectedFieldVersion: field.version,
          value: request.value,
        });
      }
    } catch (err) {
      setMoveError(problemMessage(err, "collection.saveError"));
    } finally {
      await refresh();
      setMoving(false);
    }
  }

  // Same rules as the board: no optimistic update, the server decides (versions
  // / expectedDates guard concurrent edits) and a rejection stays visible.
  async function moveToDate(row: CalendarRow, target: string | null) {
    const request = dateMoveRequest(config.dateBy, row, target, fields.data?.items ?? [], timeZone);
    if (!request || moving) return;
    setMoveError(null);
    if (request.kind === "unavailable") {
      setMoveError(t("collection.saveError"));
      return;
    }
    setMoving(true);
    try {
      if (request.kind === "task") {
        await ensureOk(
          await api.PATCH("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
            params: { path: { workspace_id: workspaceId, task_id: request.taskId } },
            body: request.body,
          }),
        );
        await queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] });
      } else {
        await putCollectionValue(workspaceId, collectionId, row.id, {
          fieldId: request.fieldId,
          expectedVersion: request.expectedVersion,
          expectedFieldVersion: request.expectedFieldVersion,
          value: request.value,
        });
      }
    } catch (err) {
      setMoveError(problemMessage(err, "collection.saveError"));
    } finally {
      await refresh();
      setMoving(false);
    }
  }

  const saveView = useMutation({
    mutationFn: async () => {
      const payload = {
        name: viewName.trim(),
        type,
        visibility,
        config: asJsonObject(config),
      };
      if (view) {
        return ensureOk(
          await api.PATCH(
            "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
            {
              params: {
                path: { workspace_id: workspaceId, collection_id: collectionId, view_id: view.id },
              },
              body: { ...payload, expectedVersion: view.version },
            },
          ),
        );
      }
      return ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views", {
          params: { path: { workspace_id: workspaceId, collection_id: collectionId } },
          body: payload,
        }),
      );
    },
    onError: async (err) => {
      if (err instanceof ProblemError && err.status === 409) {
        setViewConflict(true);
        await views.refetch();
      }
    },
    onSuccess: async (saved) => {
      setView(saved);
      onOpenView(type, saved.id);
      await refresh();
    },
  });

  const removeView = useMutation({
    mutationFn: async (viewId: string) =>
      ensureOk(
        await api.DELETE(
          "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
          {
            params: {
              path: { workspace_id: workspaceId, collection_id: collectionId, view_id: viewId },
            },
          },
        ),
      ),
    onSuccess: async () => {
      applyView(null);
      onOpenView(type, null);
      await refresh();
    },
  });

  const failed = fields.isError || views.isError || me.isError;
  if (failed) {
    return (
      <QueryError
        message={t("collection.error")}
        onRetry={() => {
          void fields.refetch();
          void views.refetch();
          void me.refetch();
        }}
      />
    );
  }
  if (!fields.data || !views.data || !me.data) return <p role="status">{t("collection.loading")}</p>;

  const memberItems: MemberOutput[] = members.data?.items ?? [];
  const userNames = memberItems.map((member) => ({
    userId: member.userId,
    name: formatPersonName(member),
  }));
  const active = fields.data.items.filter((field) => field.deletedAt === null);
  const canSave = views.data.canSave;
  const canManageViews = views.data.canManage;
  const ownsView = view === null || view.ownerId === me.data.userId;
  const shareBlocked = (visibility === "shared" || view?.visibility === "shared") && !canManageViews;
  // A stale cursor resets above; it must not disable saving the view.
  const invalidQuery = isInvalidInput(rows.error);

  const sortItems = [
    { id: "created", name: t("collection.created") },
    { id: "title", name: t("collection.resourceTitle") },
    ...active
      .filter((field) => SORTABLE_FIELD_TYPES.includes(field.type))
      .map((field) => ({ id: field.id, name: field.name })),
  ];
  const primary = readPrimarySort(config.query);
  const simple = primary && sortItems.some((item) => item.id === primary.field) ? primary : null;
  const sortValue = simple?.field ?? (config.query.sort.length > 0 ? "advanced" : "default");

  function formatValue(field: CollectionField, raw: unknown): string {
    return formatCollectionValue(
      asCollectionValue(raw),
      field.options,
      userNames,
      timeZone,
      { yes: t("collection.filter.true"), no: t("collection.filter.false") },
    );
  }

  function valueOf(row: { values: Record<string, never> }, fieldId: string): unknown {
    return (row.values as Record<string, unknown>)[fieldId];
  }

  const titleLink = (row: { displayId: string; title: string }, draggable?: false) => (
    <Link
      to={itemPath(slug, row.displayId)}
      className="font-medium hover:underline"
      draggable={draggable}
    >
      <span className="text-muted-foreground">{row.displayId}</span> {row.title}
    </Link>
  );

  const calendarDrag = type === "calendar" && config.dateBy !== null;
  const canMoveDate = (row: CalendarRow | null, target: string | null) =>
    !moving &&
    row !== null &&
    dateMoveRequest(config.dateBy, row, target, active, timeZone) !== null;
  const dragSource = (row: CalendarRow) =>
    calendarDrag && !moving && dateMovable(config.dateBy, row, active)
      ? {
          draggable: true,
          onDragStart: (event: DragEvent) => {
            event.dataTransfer.setData(CALENDAR_DRAG_TYPE, row.id);
            event.dataTransfer.effectAllowed = "move";
            draggedDate.current = row;
          },
          onDragEnd: () => {
            draggedDate.current = null;
            setDropDay(undefined);
          },
        }
      : {};
  const dropTarget = (target: string | null) => ({
    "data-drop-over": dropDay === target ? "true" : undefined,
    onDragOver: (event: DragEvent) => {
      if (!canMoveDate(draggedDate.current, target)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      if (dropDay !== target) setDropDay(target);
    },
    onDragLeave: (event: DragEvent) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropDay(undefined);
    },
    onDrop: (event: DragEvent) => {
      const row = draggedDate.current;
      draggedDate.current = null;
      setDropDay(undefined);
      if (!row || !canMoveDate(row, target)) return;
      if (event.dataTransfer.getData(CALENDAR_DRAG_TYPE) !== row.id) return;
      event.preventDefault();
      void moveToDate(row, target);
    },
  });

  const table = (items: readonly CollectionQueryItem[]) => (
    <div className="data-table-wrap">
      <table className="data-table" data-testid="collection-table">
        <thead>
          <tr>
            <th scope="col">{t("collection.resourceTitle")}</th>
            {active.map((field) => (
              <th scope="col" key={field.id}>
                {field.name}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {items.map((row) => (
            <tr key={row.id} data-testid={`collection-row-${row.displayId}`}>
              <td
                className="min-w-40"
                data-testid={calendarDrag ? `collection-drag-${row.displayId}` : undefined}
                aria-busy={calendarDrag && moving ? true : undefined}
                {...dragSource(row)}
              >
                {titleLink(row, calendarDrag ? false : undefined)}
              </td>
              {active.map((field) => (
                <td key={field.id} className="min-w-44">
                  <ValueEditor
                    field={field}
                    value={asCollectionValue(valueOf(row, field.id))}
                    members={memberItems}
                    timeZone={timeZone}
                    readOnly={!row.canEdit}
                    showLabel={false}
                    labelSuffix={row.displayId}
                    onSave={(value) => setValue(row, field, value)}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );

  const cardContent = (row: CollectionQueryItem) => (
    <>
      {titleLink(row)}
      {active.map((field) => {
        const text = formatValue(field, valueOf(row, field.id));
        return text ? (
          <p key={field.id} className="collection-card__meta">
            {field.name}: {text}
          </p>
        ) : null;
      })}
    </>
  );

  const card = (row: CollectionQueryItem) => (
    <li key={row.id} className="collection-card" data-testid={`collection-card-${row.displayId}`}>
      {cardContent(row)}
    </li>
  );

  const dayCounts = new Map((rows.data?.days ?? []).map((entry) => [entry.date, entry.count]));
  const previewsByDay = new Map<string, CollectionQueryPreview[]>();
  for (const preview of rows.data?.previews ?? []) {
    if (!preview.date) continue;
    const list = previewsByDay.get(preview.date) ?? [];
    list.push(preview);
    previewsByDay.set(preview.date, list);
  }
  const today = todayInTimeZone(timeZone);
  const monthLabel = t("cal.yearMonth", {
    year: effectiveMonth.slice(0, 4),
    month: Number(effectiveMonth.slice(5, 7)),
  });

  const calendar = (
    <div className="flex flex-col gap-2">
      <div className="collection-toolbar">
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => {
            setMonth(shiftMonth(effectiveMonth, -1));
            setDay(undefined);
            setCursor(undefined);
          }}
        >
          {t("cal.prevMonth")}
        </Button>
        <div className="collection-field">
          <label htmlFor={`${baseId}-month`}><span className="sr-only">{monthLabel}</span></label>
          <Input
            id={`${baseId}-month`}
            className="h-9"
            type="month"
            value={effectiveMonth}
            onChange={(event) => {
              if (!isMonth(event.target.value)) return;
              setMonth(event.target.value);
              setDay(undefined);
              setCursor(undefined);
            }}
          />
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => {
            setMonth(shiftMonth(effectiveMonth, 1));
            setDay(undefined);
            setCursor(undefined);
          }}
        >
          {t("cal.nextMonth")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => {
            setMonth(today.slice(0, 7));
            setDay(undefined);
            setCursor(undefined);
          }}
        >
          {t("cal.today")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          aria-pressed={day === null}
          className="collection-calendar__none"
          {...dropTarget(null)}
          onClick={() => {
            setDay(day === null ? undefined : null);
            setCursor(undefined);
          }}
        >
          {t("collection.unassigned")} · {dayCounts.get(null) ?? 0}
        </Button>
      </div>
      <table className="collection-calendar" aria-label={monthLabel} data-testid="collection-calendar">
        <thead>
          <tr>
            {weekdayNames(weekStartsOn).map((name) => (
              <th key={name} scope="col">
                {name}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {monthGrid(effectiveMonth, weekStartsOn).map((week) => (
            <tr key={week[0]!.date}>
              {week.map((cell) => {
                const count = dayCounts.get(cell.date) ?? 0;
                const previews = previewsByDay.get(cell.date) ?? [];
                return (
                  <td
                    key={cell.date}
                    data-outside={cell.inMonth ? undefined : "true"}
                    data-date={cell.date}
                    {...dropTarget(cell.date)}
                  >
                    {cell.inMonth ? (
                      <>
                        <button
                          type="button"
                          className="collection-calendar__day"
                          aria-pressed={day === cell.date}
                          data-today={cell.date === today ? "true" : undefined}
                          aria-label={`${cell.date} · ${t("collection.count", { count })}`}
                          onClick={() => {
                            setDay(day === cell.date ? undefined : cell.date);
                            setCursor(undefined);
                          }}
                        >
                          <span>{Number(cell.date.slice(8, 10))}</span>
                          {count > 0 ? <span>{count}</span> : null}
                        </button>
                        {previews.length > 0 ? (
                          <ul className="collection-calendar__previews">
                            {previews.map((preview) => (
                              <li
                                key={preview.id}
                                data-testid={`collection-preview-${preview.displayId}`}
                                aria-busy={moving || undefined}
                                {...dragSource(preview)}
                              >
                                <Link
                                  to={itemPath(slug, preview.displayId)}
                                  title={preview.title}
                                  draggable={false}
                                >
                                  {preview.title}
                                </Link>
                              </li>
                            ))}
                            {count > previews.length ? (
                              <li className="text-muted-foreground">
                                {t("collection.more", { count: count - previews.length })}
                              </li>
                            ) : null}
                          </ul>
                        ) : null}
                      </>
                    ) : null}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );

  return (
    <section className="flex min-w-0 flex-col gap-4" data-testid={`collection-${type}`}>
      <div className="collection-toolbar">
        <div className="collection-field">
          <label htmlFor={`${baseId}-view`}>{t("collection.savedViews")}</label>
          <select
            id={`${baseId}-view`}
            className="collection-select"
            value={view?.id ?? ""}
            disabled={removeView.isPending}
            onChange={(event) => {
              const saved = views.data?.items.find((item) => item.id === event.target.value) ?? null;
              saveView.reset();
              if (saved && isViewType(saved.type) && saved.type !== type) {
                onOpenView(saved.type, saved.id);
                return;
              }
              if (saved) applyView(saved);
              else applyView(null);
              onOpenView(type, saved?.id ?? null);
            }}
          >
            <option value="">{t("collection.newView")}</option>
            {views.data.items.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name} ·{" "}
                {item.visibility === "shared" ? t("collection.shared") : t("collection.private")}
              </option>
            ))}
          </select>
        </div>
        {type === "board" ? (
          <div className="collection-field">
            <label htmlFor={`${baseId}-group`}>{t("collection.group")}</label>
            <select
              id={`${baseId}-group`}
              className="collection-select"
              value={config.groupBy ?? ""}
              onChange={(event) => change({ groupBy: event.target.value || null })}
            >
              <option value="">{t("collection.none")}</option>
              <option value="status">{t("collection.status")}</option>
              {active
                .filter((field) => field.type === "select")
                .map((field) => (
                  <option key={field.id} value={field.id}>
                    {field.name}
                  </option>
                ))}
            </select>
          </div>
        ) : null}
        {type === "calendar" ? (
          <div className="collection-field">
            <label htmlFor={`${baseId}-date`}>{t("collection.date")}</label>
            <select
              id={`${baseId}-date`}
              className="collection-select"
              value={config.dateBy ?? ""}
              onChange={(event) => change({ dateBy: event.target.value || null })}
            >
              <option value="">{t("collection.none")}</option>
              <option value="due">{t("collection.due")}</option>
              <option value="start">{t("collection.start")}</option>
              {active
                .filter((field) => field.type === "date" || field.type === "datetime")
                .map((field) => (
                  <option key={field.id} value={field.id}>
                    {field.name}
                  </option>
                ))}
            </select>
          </div>
        ) : null}
        <div className="collection-field">
          <label htmlFor={`${baseId}-sort`}>{t("collection.sort")}</label>
          <select
            id={`${baseId}-sort`}
            className="collection-select"
            value={sortValue}
            onChange={(event) => {
              const value = event.target.value;
              change({
                query: setPrimarySort(
                  config.query,
                  value === "default" || value === "advanced" ? null : value,
                  simple?.direction ?? "asc",
                ),
              });
            }}
          >
            <option value="default">{t("collection.sort.default")}</option>
            {sortValue === "advanced" ? (
              <option value="advanced">{t("collection.sort.advanced")}</option>
            ) : null}
            {sortItems.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
              </option>
            ))}
          </select>
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!simple}
          onClick={() => {
            if (!simple) return;
            change({
              query: setPrimarySort(
                config.query,
                simple.field,
                simple.direction === "asc" ? "desc" : "asc",
              ),
            });
          }}
        >
          {simple?.direction === "desc" ? t("collection.descending") : t("collection.ascending")}
        </Button>
        {active.length > 0 ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            aria-expanded={customOpen}
            onClick={() => setCustomOpen((open) => !open)}
          >
            {t("collection.filter.custom")}
          </Button>
        ) : null}
      </div>
      {customOpen && active.length > 0 ? (
        <CustomFilters
          fields={active}
          members={memberItems}
          timeZone={timeZone}
          query={config.query}
          onQueryChange={(next) => change({ query: next })}
        />
      ) : null}
      {canSave ? (
        <form
          className="collection-toolbar"
          onSubmit={(event) => {
            event.preventDefault();
            if (!viewName.trim() || saveView.isPending || viewConflict || shareBlocked || invalidQuery) {
              return;
            }
            saveView.mutate();
          }}
        >
          <div className="collection-field">
            <label htmlFor={`${baseId}-view-name`}>{t("collection.viewName")}</label>
            <Input
              id={`${baseId}-view-name`}
              className="h-9"
              maxLength={100}
              value={viewName}
              onChange={(event) => setViewName(event.target.value)}
            />
          </div>
          <div className="collection-field">
            <label htmlFor={`${baseId}-visibility`}>{t("collection.visibility")}</label>
            <select
              id={`${baseId}-visibility`}
              className="collection-select"
              value={visibility}
              disabled={!ownsView}
              onChange={(event) =>
                setVisibility(event.target.value === "shared" ? "shared" : "private")
              }
            >
              <option value="private">{t("collection.private")}</option>
              <option value="shared" disabled={!canManageViews}>
                {t("collection.shared")}
              </option>
            </select>
          </div>
          <Button
            type="submit"
            size="sm"
            disabled={
              !viewName.trim() ||
              saveView.isPending ||
              removeView.isPending ||
              viewConflict ||
              shareBlocked ||
              invalidQuery
            }
          >
            {t("collection.saveView")}
          </Button>
          {view ? (
            <ConfirmActionButton
              title={t("collection.deleteView")}
              description={t("collection.deleteView.confirm")}
              actionLabel={t("project.views.delete")}
              disabled={
                removeView.isPending ||
                (view.visibility === "shared" ? !canManageViews : view.ownerId !== me.data.userId)
              }
              onConfirm={async () => {
                try {
                  await removeView.mutateAsync(view.id);
                } catch {
                  /* removeView.isError shows the message. */
                }
              }}
            >
              {t("collection.deleteView")}
            </ConfirmActionButton>
          ) : null}
        </form>
      ) : null}
      {viewConflict ? (
        <div role="alert" className="collection-toolbar">
          <span className="text-ui text-destructive">{t("collection.viewConflict")}</span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={views.isFetching}
            onClick={() => {
              const latest = views.data?.items.find((item) => item.id === view?.id) ?? null;
              saveView.reset();
              applyView(latest);
            }}
          >
            {t("collection.reloadView")}
          </Button>
        </div>
      ) : null}
      {(saveView.isError && !viewConflict) || removeView.isError ? (
        <p role="alert" className="text-ui text-destructive">
          {t("collection.saveError")}
        </p>
      ) : null}
      {moveError ? (
        <p role="alert" className="text-ui text-destructive">
          {moveError}
        </p>
      ) : null}

      {type === "calendar" && config.dateBy ? calendar : null}

      {rows.isPending ? <QueryLoading /> : null}
      {rows.isError ? (
        <QueryError
          message={loadErrorMessage(rows.error)}
          onRetry={() => void rows.refetch()}
        />
      ) : null}
      {rows.data ? (
        <>
          <p className="text-caption text-muted-foreground" role="status">
            {t("collection.count", { count: rows.data.count })}
          </p>
          {type === "calendar" && config.dateBy && day === undefined ? null : (groupedBoard
              ? rows.data.count
              : rows.data.items.length) === 0 ? (
            <p className="text-ui text-muted-foreground">{t("collection.empty")}</p>
          ) : groupedBoard ? (
            <CollectionBoard
              workspaceId={workspaceId}
              collectionId={collectionId}
              config={config}
              groups={rows.data.groups}
              moving={moving}
              renderContent={cardContent}
              onMove={moveToGroup}
            />
          ) : type === "board" ? (
            <ul className="flex flex-col gap-2">{rows.data.items.map(card)}</ul>
          ) : (
            <>
              {type === "calendar" ? (
                <h3 className="text-ui font-semibold">
                  {t("collection.dayItems", { date: day ?? t("collection.unassigned") })}
                </h3>
              ) : null}
              {table(rows.data.items)}
            </>
          )}
          {!groupedBoard && (cursor || rows.data.nextCursor) ? (
            <div className="collection-toolbar">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!cursor}
                onClick={() => setCursor(undefined)}
              >
                {t("collection.previous")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!rows.data.nextCursor}
                onClick={() => setCursor(rows.data?.nextCursor ?? undefined)}
              >
                {t("collection.next")}
              </Button>
            </div>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
