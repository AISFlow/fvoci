import { notificationMessage, t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { QueryError, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import {
  notificationHref,
  payloadRecord,
  type NotificationItem,
} from "@/features/notifications/notification-target";
import { api, ensureOk } from "@/lib/api";
import { notificationsPath } from "@/lib/href";
import { notificationListQuery, notificationUnreadCountQuery } from "@/lib/queries";
import "@/features/notifications/notifications.css";

export function NotificationBell({
  slug,
  workspaceId,
}: {
  slug: string;
  workspaceId: string;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const unread = useQuery(notificationUnreadCountQuery(workspaceId));
  const list = useQuery({
    ...notificationListQuery(workspaceId, "all"),
    enabled: open,
  });
  const count = unread.data?.count ?? 0;
  const items = list.data?.items ?? [];

  async function markRead(item: NotificationItem) {
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
    setOpen(false);
    if (href) void navigate(href);
  }

  async function readAll() {
    await ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/notifications/read-all", {
        params: { path: { workspace_id: workspaceId } },
      }),
    );
    await queryClient.invalidateQueries({ queryKey: ["notifications", workspaceId] });
    await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspaceId] });
  }

  return (
    <div className="notification-bell">
      <button
        type="button"
        className="notification-bell__trigger"
        aria-expanded={open}
        aria-label={count > 0 ? t("notif.unreadCount", { count }) : t("notif.bell.title")}
        onClick={() => setOpen((value) => !value)}
      >
        <span aria-hidden="true">🔔</span>
        {count > 0 ? (
          <span className="notification-bell__badge" aria-hidden="true">
            {count > 99 ? "99+" : count}
          </span>
        ) : null}
      </button>
      {open ? (
        <div className="notification-bell__panel" aria-label={t("notif.bell.title")}>
          <div className="notification-bell__head">
            <p className="notification-bell__title">{t("notif.bell.title")}</p>
            {count > 0 ? (
              <Button type="button" variant="outline" size="sm" onClick={() => void readAll()}>
                {t("notif.readAll")}
              </Button>
            ) : null}
          </div>
          {list.isPending ? (
            <p className="notification-bell__empty">{t("load.loading")}</p>
          ) : null}
          {list.isError ? (
            <div className="notification-bell__empty">
              <QueryError
                message={loadErrorMessage(list.error)}
                onRetry={() => {
                  void list.refetch();
                }}
              />
            </div>
          ) : null}
          {!list.isPending && !list.isError && items.length === 0 ? (
            <p className="notification-bell__empty">{t("notif.empty")}</p>
          ) : null}
          {items.length > 0 ? (
            <ul className="notification-bell__list">
              {items.map((item) => (
                <li key={item.id}>
                  <button
                    type="button"
                    className="notification-bell__item"
                    onClick={() => void markRead(item)}
                  >
                    <span
                      className={
                        item.readAt
                          ? "notification-bell__dot is-read"
                          : "notification-bell__dot"
                      }
                      aria-hidden="true"
                    />
                    <span>{notificationMessage({ verb: item.verb, payload: payloadRecord(item.payload) })}</span>
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          <div className="notification-bell__foot">
            <Link to={notificationsPath(slug)} onClick={() => setOpen(false)}>
              {t("notif.viewAll")}
            </Link>
          </div>
        </div>
      ) : null}
    </div>
  );
}
