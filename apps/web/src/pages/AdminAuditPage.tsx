// Adapted from source apps/web/src/routes/settings.audit.tsx.
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { AdminShell } from "@/features/settings/admin-shell";
import { AuditSettingsView } from "@/features/settings/settings-audit";
import { ProblemError } from "@/lib/api";
import { FALLBACK_TZ } from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import { adminAuditQuery } from "@/lib/queries/admin";

function AuditLog() {
  const me = useQuery(meQuery);
  const audit = useQuery(adminAuditQuery);
  // A 404 here is the enterprise gate (the source hides audit without a license).
  const eeRequired = audit.error instanceof ProblemError && audit.error.status === 404;
  const failure = !eeRequired && audit.error
    ? audit.error instanceof ProblemError
      ? audit.error.title
      : t("error.network")
    : null;
  return (
    <AuditSettingsView
      items={audit.data?.items ?? []}
      timeZone={me.data?.timezone ?? FALLBACK_TZ}
      loading={audit.isLoading}
      eeRequired={eeRequired}
      error={failure}
    />
  );
}

export function AdminAuditPage() {
  return (
    <AdminShell active="audit">
      <AuditLog />
    </AdminShell>
  );
}
