// Grouped collection board: each column pages its own group through the
// collection query (`group` + cursor) and cards move by native drag-and-drop
// or the per-card group select. Adapted from source collection-cards.tsx and
// collection-drag.tsx (Atlaskit pragmatic-drag-and-drop wraps the same native
// events; no extra dependency here).
import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { api, ensureOk, ProblemError } from "@/lib/api";
import {
  asJsonObject,
  collectionPrefix,
  type CollectionConfig,
  type CollectionQueryItem,
} from "@/lib/queries/collections";
import {
  BOARD_DRAG_TYPE,
  boardColumnBody,
  columnRows,
  moveChoices,
  moveRequest,
  type BoardGroup,
} from "./board-model";

type BoardProps = {
  workspaceId: string;
  collectionId: string;
  config: CollectionConfig;
  groups: readonly BoardGroup[];
  /** True while a move request is in flight; cards cannot be moved again. */
  moving: boolean;
  renderContent: (row: CollectionQueryItem) => ReactNode;
  onMove: (row: CollectionQueryItem, target: BoardGroup) => Promise<void>;
};

function groupName(group: BoardGroup): string {
  return group.name || t("collection.unassigned");
}

export function CollectionBoard(props: BoardProps) {
  const dragging = useRef<CollectionQueryItem | null>(null);
  const [over, setOver] = useState<string | null | undefined>(undefined);
  return (
    <div className="collection-board" data-testid="collection-board">
      {props.groups.map((group) => (
        <BoardColumn
          key={group.id ?? "none"}
          {...props}
          group={group}
          dragging={dragging}
          over={over === group.id}
          setOver={setOver}
        />
      ))}
    </div>
  );
}

function BoardColumn({
  workspaceId,
  collectionId,
  config,
  groups,
  moving,
  renderContent,
  onMove,
  group,
  dragging,
  over,
  setOver,
}: BoardProps & {
  group: BoardGroup;
  dragging: { current: CollectionQueryItem | null };
  over: boolean;
  setOver: (group: string | null | undefined) => void;
}) {
  const queryClient = useQueryClient();
  const queryKey = [
    ...collectionPrefix(workspaceId, collectionId),
    "board",
    config,
    group.id,
  ] as const;
  const pages = useInfiniteQuery({
    queryKey,
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) => {
      const body = boardColumnBody(config, group.id, pageParam);
      return ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/query", {
          params: { path: { workspace_id: workspaceId, collection_id: collectionId } },
          body: { ...body, config: asJsonObject(body.config) },
        }),
      );
    },
    getNextPageParam: (last) => last.nextCursor ?? undefined,
    retry: false,
  });

  // A cursor from an older snapshot (fields/catalog changed) restarts only this column.
  const invalidCursor = pages.error instanceof ProblemError && pages.error.code === "invalid_cursor";
  useEffect(() => {
    if (invalidCursor) void queryClient.resetQueries({ queryKey, exact: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [invalidCursor]);

  const rows = columnRows(pages.data?.pages ?? [], group.id);
  const choices = moveChoices(config.groupBy, groups);
  const accepts = (row: CollectionQueryItem | null) =>
    !moving && row !== null && moveRequest(config.groupBy, row, group) !== null;

  const card = (row: CollectionQueryItem) => {
    const movable = row.canEdit && !moving && choices.length > 0;
    return (
      <li
        key={row.id}
        className="collection-card"
        data-testid={`collection-card-${row.displayId}`}
        draggable={movable}
        aria-busy={moving || undefined}
        onDragStart={(event) => {
          if (!movable) return;
          event.dataTransfer.setData(BOARD_DRAG_TYPE, row.id);
          event.dataTransfer.effectAllowed = "move";
          dragging.current = row;
        }}
        onDragEnd={() => {
          dragging.current = null;
          setOver(undefined);
        }}
      >
        {renderContent(row)}
        {row.canEdit && choices.length > 0 ? (
          <select
            className="collection-select"
            aria-label={`${t("collection.group")} · ${row.displayId}`}
            value={row.group ?? ""}
            disabled={moving}
            onChange={(event) => {
              const target = groups.find((item) => (item.id ?? "") === event.target.value);
              if (target) void onMove(row, target);
            }}
          >
            {choices.map((choice) => (
              <option
                key={choice.id ?? "none"}
                value={choice.id ?? ""}
                disabled={choice.disabled && choice.id !== row.group}
              >
                {choice.name || t("collection.unassigned")}
                {choice.disabled ? ` · ${t("collection.archived")}` : ""}
              </option>
            ))}
          </select>
        ) : null}
      </li>
    );
  };

  return (
    <section
      className="collection-board__column"
      aria-label={groupName(group)}
      data-testid={`collection-group-${group.name || "none"}`}
      data-drop-over={over ? "true" : undefined}
      onDragOver={(event) => {
        if (!accepts(dragging.current)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
        if (!over) setOver(group.id);
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOver(undefined);
      }}
      onDrop={(event) => {
        const row = dragging.current;
        dragging.current = null;
        setOver(undefined);
        if (!row || !accepts(row) || event.dataTransfer.getData(BOARD_DRAG_TYPE) !== row.id) return;
        event.preventDefault();
        void onMove(row, group);
      }}
    >
      <h3 className="collection-board__head">
        <span>
          {groupName(group)}
          {group.deleted ? ` · ${t("collection.archived")}` : ""}
        </span>
        <span className="text-muted-foreground">{group.count}</span>
      </h3>
      {pages.isPending ? <QueryLoading /> : null}
      {pages.isError && !invalidCursor && !pages.isFetchNextPageError ? (
        <QueryError message={loadErrorMessage(pages.error)} onRetry={() => void pages.refetch()} />
      ) : null}
      {pages.data ? (
        rows.length === 0 ? (
          <p className="text-ui text-muted-foreground">{t("collection.emptyPage")}</p>
        ) : (
          <ul className="flex flex-col gap-2">{rows.map(card)}</ul>
        )
      ) : null}
      {pages.isFetchNextPageError && !invalidCursor ? (
        <p role="alert" className="text-ui text-destructive">
          {t("search.loadMoreError")}
        </p>
      ) : null}
      {pages.hasNextPage ? (
        <Button
          type="button"
          size="sm"
          variant="outline"
          aria-label={`${groupName(group)} · ${t("search.loadMore")}`}
          disabled={pages.isFetchingNextPage}
          onClick={() => void pages.fetchNextPage()}
        >
          {t("search.loadMore")}
        </Button>
      ) : null}
    </section>
  );
}
