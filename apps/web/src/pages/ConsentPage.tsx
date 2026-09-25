// Adapted from source apps/web/src/routes/consent.tsx.
import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { Navigate } from "react-router-dom";
import { loadErrorMessage, QueryError, QueryLoading } from "@/components/query-status";
import { ConsentView } from "@/features/auth/consent";
import { AuthLayout } from "@/features/auth/auth-layout";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { safeReturnTo } from "@/lib/consent";

export function ConsentPage() {
  const returnTo = safeReturnTo(
    new URLSearchParams(window.location.search).get("returnTo"),
    window.location.origin,
  );
  const pending = useQuery({
    queryKey: ["consents-pending"],
    queryFn: async () => (await ensureOk(await api.GET("/api/v1/auth/consents/pending"))).pending,
    retry: false,
    staleTime: 0,
  });

  // Nothing (left) to accept: continue where the gate interrupted.
  useEffect(() => {
    if (pending.data !== undefined && pending.data.length === 0) {
      window.location.assign(returnTo);
    }
  }, [pending.data, returnTo]);

  if (pending.error instanceof ProblemError && pending.error.status === 401) {
    return <Navigate to="/login" replace />;
  }
  if (pending.isError) {
    return (
      <AuthLayout>
        <QueryError message={loadErrorMessage(pending.error)} onRetry={() => void pending.refetch()} />
      </AuthLayout>
    );
  }
  if (!pending.data || pending.data.length === 0) {
    return (
      <AuthLayout>
        <QueryLoading />
      </AuthLayout>
    );
  }

  return (
    <ConsentView
      pending={pending.data}
      onSubmit={async (items) => {
        await ensureOk(await api.POST("/api/v1/auth/consents", { body: { items } }));
        // A full load drops every query that failed on the gate.
        window.location.assign(returnTo);
      }}
    />
  );
}
