import { notificationMessage, t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, toValue, type MaybeRefOrGetter } from "vue";
import {
  markAllNotificationsRead,
  markNotificationRead,
} from "@/features/notifications/notification-actions";
import {
  notificationHref,
  payloadRecord,
  type NotificationItem,
} from "@/features/notifications/notification-target";
import { notificationListQuery, notificationUnreadCountQuery } from "@/lib/queries";

/**
 * The header bell (features/notifications/notification-bell.tsx): the unread
 * count polls every 30 s (notificationUnreadCountQuery) and the latest
 * notifications load while the panel is open. Opening one marks it read
 * first (unless it already is), then closes the panel and goes to its item.
 */
export function useNotificationBell(options: {
  workspaceId: MaybeRefOrGetter<string>;
  slug: MaybeRefOrGetter<string>;
  navigate: (path: string) => void;
}) {
  const queryClient = useQueryClient();
  const open = ref(false);
  const unread = useQuery(() => notificationUnreadCountQuery(toValue(options.workspaceId)));
  const list = useQuery(() => ({
    ...notificationListQuery(toValue(options.workspaceId), "all"),
    enabled: open.value,
  }));

  const count = computed(() => unread.data.value?.count ?? 0);
  const items = computed<NotificationItem[]>(() => list.data.value?.items ?? []);
  /** The trigger's accessible name: the unread count, or the bell's title. */
  const label = computed(() =>
    count.value > 0 ? t("notif.unreadCount", { count: count.value }) : t("notif.bell.title"),
  );
  const badge = computed(() => (count.value > 99 ? "99+" : String(count.value)));

  function message(item: NotificationItem): string {
    return notificationMessage({ verb: item.verb, payload: payloadRecord(item.payload) });
  }

  /** Rejects (and stays on the page, panel open) when marking it read fails. */
  async function openItem(item: NotificationItem): Promise<void> {
    if (!item.readAt) {
      await markNotificationRead(queryClient, toValue(options.workspaceId), item.id);
    }
    const href = notificationHref(toValue(options.slug), item);
    open.value = false;
    if (href) options.navigate(href);
  }

  function readAll(): Promise<void> {
    return markAllNotificationsRead(queryClient, toValue(options.workspaceId));
  }

  return { open, unread, list, count, items, label, badge, message, openItem, readAll };
}
