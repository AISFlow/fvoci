import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { loadErrorMessage } from "@/components/query-status";
import { FALLBACK_TZ } from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import { workspaceEventsQuery } from "@/lib/queries/workspace";
import { WorkspaceEventsView } from "./workspace-events-view";
import "./settings-shell.css";

/** Source settings "활동": workspace owners/admins read the event log. */
export function WorkspaceEventsSection({ workspaceId }: { workspaceId: string }) {
  const me = useQuery(meQuery);
  const events = useInfiniteQuery(workspaceEventsQuery(workspaceId));
  const items = useMemo(
    () => events.data?.pages.flatMap((page) => page.items) ?? [],
    [events.data],
  );
  // A failed later page keeps the rows already shown and reports under "load more".
  const firstPageFailed = events.isError && !events.isFetchNextPageError;
  return (
    <WorkspaceEventsView
      items={items}
      timeZone={me.data?.timezone ?? FALLBACK_TZ}
      loading={events.isLoading}
      error={firstPageFailed ? loadErrorMessage(events.error) : null}
      onRetry={() => {
        void events.refetch();
      }}
      hasMore={events.hasNextPage}
      loadingMore={events.isFetchingNextPage}
      loadMoreError={events.isFetchNextPageError ? t("settings.activity.loadMoreFailed") : null}
      onLoadMore={() => {
        void events.fetchNextPage();
      }}
    />
  );
}
