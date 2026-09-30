// Adapted from source apps/web/src/features/settings/settings-admin.tsx: the
// admin console's messages and the erasure countdown, framework-neutral.
import { t } from "@fvoci/i18n";
import { ProblemError } from "@/lib/api";

/** Codes the shared problem table does not title; this screen owns their wording. */
export function adminActionMessage(err: unknown): string {
  if (err instanceof ProblemError) {
    if (err.code === "last_instance_admin") return t("admin.lastAdmin");
    if (err.code === "self_suspension") return t("self_suspension");
    return err.title;
  }
  return t("error.network");
}

/** Whole days left until `iso` (source `daysUntil`); 0 once the deadline passed. */
export function daysUntil(iso: string, now: number = Date.now()): number {
  return Math.max(0, Math.ceil((Date.parse(iso) - now) / 86_400_000));
}
