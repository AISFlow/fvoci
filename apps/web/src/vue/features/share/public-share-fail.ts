import { t } from "@fvoci/i18n";
import { ProblemError } from "@/lib/api";

/** React `PublicSharePage` `failMessage`: 404, known problem title, else share/network. */
export function failMessage(err: unknown): string {
  if (err instanceof ProblemError && err.status === 404) return t("share.expired");
  if (err instanceof ProblemError && err.titleKnown) return err.title;
  if (err instanceof ProblemError) return t("error.share.failed");
  return t("error.network");
}
