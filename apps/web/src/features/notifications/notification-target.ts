import type { components } from "@/generated/api";
import { itemPath } from "@/lib/href";

export type NotificationItem = components["schemas"]["NotificationItemOutput"];

export function notificationHref(slug: string, item: NotificationItem): string | null {
  if (!item.displayId) return null;
  return itemPath(slug, item.displayId);
}

export function payloadRecord(payload: unknown): Record<string, unknown> {
  return payload !== null && typeof payload === "object" && !Array.isArray(payload)
    ? (payload as Record<string, unknown>)
    : {};
}
