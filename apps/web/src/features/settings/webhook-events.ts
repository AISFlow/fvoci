import { isI18nKey, t, type I18nKey } from "@fvoci/i18n";

/** Source `WEBHOOK_EVENTS`: the verbs the settings form offers. */
export const WEBHOOK_EVENTS = [
  "document.created",
  "document.updated",
  "document.moved",
  "document.trashed",
  "document.restored",
  "document.purged",
  "attachment.completed",
  "attachment.deleted",
  "task.created",
  "task.updated",
  "task.deleted",
  "comment.created",
  "comment.resolved",
  "project.created",
  "project.updated",
  "project.archived",
  "project.deleted",
] as const;

export type WebhookEvent = (typeof WEBHOOK_EVENTS)[number];

/** A stored row may carry a verb outside the offered list — show it verbatim then. */
export function webhookEventLabel(verb: string): string {
  const key = `webhook.event.${verb}`;
  return isI18nKey(key) ? t(key) : verb;
}

/**
 * Maps a webhook create problem to a specific Korean message. `integration_unavailable`
 * means the server has no ENCRYPTION_KEYS to seal the secret; an `invalid_input` on `/url`
 * means the server's outbound policy refused the target.
 */
export function webhookCreateProblemKey(
  code: string | undefined,
  source: string | null | undefined,
): I18nKey | null {
  if (code === "integration_unavailable") return "webhook.unavailable";
  if (code === "invalid_input" && source === "/url") return "webhook.url.refused";
  if (code === "invalid_input" && source === "/events") return "webhook.events.required";
  return null;
}
