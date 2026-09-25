// Adapted from source apps/web/src/routes/settings.legal.tsx.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { loadErrorMessage } from "@/components/query-status";
import { AdminShell } from "@/features/settings/admin-shell";
import { SettingsLegalView } from "@/features/settings/settings-legal";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { legalDocQuery, legalVersionsQuery } from "@/lib/queries/admin";

function LegalManager() {
  const queryClient = useQueryClient();
  const [kind, setKind] = useState("terms");
  const current = useQuery({ ...legalDocQuery(kind), enabled: kind.length > 0 });
  // No published version yet is a 404, not a load failure.
  const none = current.error instanceof ProblemError && current.error.status === 404;
  const failed = current.isError && !none;

  return (
    <SettingsLegalView
      kind={kind}
      onKindChange={setKind}
      current={current.isError ? null : (current.data ?? null)}
      loading={current.isLoading}
      error={failed ? loadErrorMessage(current.error) : null}
      onRetry={() => {
        void current.refetch();
      }}
      onPublish={async (input) => {
        await ensureOk(await api.POST("/api/v1/admin/legal", { body: input }));
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: legalDocQuery(input.kind).queryKey }),
          queryClient.invalidateQueries({ queryKey: legalVersionsQuery(input.kind).queryKey }),
        ]);
      }}
    />
  );
}

export function AdminLegalPage() {
  return (
    <AdminShell active="legal">
      <LegalManager />
    </AdminShell>
  );
}
