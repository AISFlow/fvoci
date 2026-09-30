import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";

// The public legal documents (terms, privacy), framework-neutral: both web
// apps read them (lib/queries/admin.ts re-exports these for the React admin
// pages).

export type LegalVersions = components["schemas"]["LegalVersionsResponse"];

export function legalDocQuery(kind: string, version?: number) {
  return queryOptions({
    queryKey: ["legal", kind, version ?? "latest"] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/legal/{kind}", {
          params: {
            path: { kind },
            query: version === undefined ? {} : { version },
          },
        }),
      ),
    retry: false,
  });
}

export function legalVersionsQuery(kind: string) {
  return queryOptions({
    queryKey: ["legal", kind, "versions"] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/legal/{kind}/versions", {
          params: { path: { kind } },
        }),
      ),
    retry: false,
  });
}

/** The version list of {@link legalVersionsQuery} (a `select` for the caller's adapter). */
export function selectLegalVersions(data: LegalVersions) {
  return data.versions;
}
