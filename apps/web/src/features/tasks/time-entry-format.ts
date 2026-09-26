import { t } from "@fvoci/i18n";

/** Source `formatDuration`: hours and minutes, seconds dropped. */
export function formatDuration(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (h > 0 && m > 0) {
    return `${t("task.time.hours", { n: h })} ${t("task.time.minutes", { n: m })}`;
  }
  if (h > 0) return t("task.time.hours", { n: h });
  return t("task.time.minutes", { n: m });
}
