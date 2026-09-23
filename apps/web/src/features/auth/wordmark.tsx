// Adapted from fvoci/FVOCI apps/web/src/features/auth/wordmark.tsx
import { t } from "@fvoci/i18n";

export function AuthWordmark({ brandingName }: { brandingName?: string | null }) {
  return (
    <div className="mb-8 flex flex-col items-center gap-2 text-center">
      <p className="break-keep text-title font-semibold text-foreground">
        {brandingName ?? t("auth.wordmark")}
      </p>
      <p className="break-keep text-ui text-muted-foreground">{t("auth.tagline")}</p>
    </div>
  );
}
