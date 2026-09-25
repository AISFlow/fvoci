import { formatPersonName, notificationMessage, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import {
  notificationHref,
  payloadRecord,
  type NotificationItem,
} from "@/features/notifications/notification-target";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { api, ensureOk } from "@/lib/api";
import type { NotificationFilter } from "@/lib/queries";
import { notificationListQuery } from "@/lib/queries";
import "@/features/notifications/notifications.css";

const TABS: { value: NotificationFilter; label: string }[] = [
  { value: "all", label: t("notif.filter.all") },
  { value: "unread", label: t("notif.filter.unread") },
  { value: "archived", label: t("notif.tab.archived") },
];

export function NotificationsPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { slug, workspace } = useWorkspaceContext();
  const [tab, setTab] = useState<NotificationFilter>("all");
  const [extra, setExtra] = useState<NotificationItem[]>([]);
  const list = useQuery(notificationListQuery(workspace?.id ?? "", tab));
  const items = [...(list.data?.items ?? []), ...extra];

  const readAll = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/notifications/read-all", {
          params: { path: { workspace_id: workspace!.id } },
        }),
      ),
    onSuccess: async () => {
      setExtra([]);
      await queryClient.invalidateQueries({ queryKey: ["notifications", workspace?.id] });
      await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspace?.id] });
    },
  });

  if (!workspace) return null;
  const workspaceId = workspace.id;
  const workspaceName = workspace.name;

  async function openItem(item: NotificationItem) {
    if (!item.readAt) {
      await ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/notifications/{id}", {
          params: { path: { workspace_id: workspaceId, id: item.id } },
          body: { read: true },
        }),
      );
      await queryClient.invalidateQueries({ queryKey: ["notifications", workspaceId] });
      await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspaceId] });
    }
    const href = notificationHref(slug, item);
    if (href) void navigate(href);
  }

  async function toggleArchive(item: NotificationItem) {
    await ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}/notifications/{id}", {
        params: { path: { workspace_id: workspaceId, id: item.id } },
        body: { archived: !item.archivedAt },
      }),
    );
    setExtra([]);
    await queryClient.invalidateQueries({ queryKey: ["notifications", workspaceId] });
    await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspaceId] });
  }

  async function loadMore() {
    const cursor = list.data?.nextCursor;
    if (!cursor) return;
    const page = await ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/notifications", {
        params: {
          path: { workspace_id: workspaceId },
          query: { filter: tab, cursor },
        },
      }),
    );
    setExtra((current) => [...current, ...page.items]);
  }

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspaceId}
      workspaceName={workspaceName}
      activeNav="notifications"
    >
      <div className="notifications-page">
        <div className="notifications-page__head">
          <h1 className="notifications-page__title">{t("notif.list.title")}</h1>
          <Button type="button" variant="outline" size="sm" onClick={() => void readAll.mutateAsync()}>
            {t("notif.readAllFull")}
          </Button>
        </div>
        <div role="tablist" aria-label={t("notif.filter")} className="notifications-page__tabs">
          {TABS.map((entry) => (
            <button
              key={entry.value}
              type="button"
              role="tab"
              aria-selected={tab === entry.value}
              className={
                tab === entry.value
                  ? "notifications-page__tab is-active"
                  : "notifications-page__tab"
              }
              onClick={() => {
                setTab(entry.value);
                setExtra([]);
              }}
            >
              {entry.label}
            </button>
          ))}
        </div>
        {list.isPending ? <QueryLoading /> : null}
        {list.isError ? (
          <QueryError
            message={loadErrorMessage(list.error)}
            onRetry={() => {
              void list.refetch();
            }}
          />
        ) : null}
        {!list.isPending && !list.isError && items.length === 0 ? (
          <p className="notifications-page__empty">
            {tab === "archived" ? t("notif.emptyArchived") : t("notif.empty")}
          </p>
        ) : null}
        {items.length > 0 ? (
          <ul className="notifications-page__list">
            {items.map((item) => (
              <li key={item.id} className="notifications-page__row">
                <button
                  type="button"
                  className="notifications-page__item"
                  onClick={() => void openItem(item)}
                >
                  <span className="notifications-page__actor">
                    {!item.readAt ? (
                      <span className="sr-only">{t("notif.filter.unread")}</span>
                    ) : null}
                    {item.actorGivenName
                      ? formatPersonName({
                          givenName: item.actorGivenName,
                          familyName: item.actorFamilyName,
                        })
                      : t("notif.unknownActor")}
                  </span>
                  <span className="notifications-page__message">
                    {notificationMessage({
                      verb: item.verb,
                      payload: payloadRecord(item.payload),
                    })}
                  </span>
                </button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => void toggleArchive(item)}
                >
                  {item.archivedAt ? t("notif.unarchive") : t("notif.archive")}
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
        {list.data?.nextCursor && extra.length === 0 ? (
          <Button type="button" variant="outline" size="sm" onClick={() => void loadMore()}>
            {t("notif.list.loadMore")}
          </Button>
        ) : null}
      </div>
    </WorkspaceShell>
  );
}
