// Adapted from source apps/web/src/lib/queries/admin.ts and the legal queries
// in apps/web/src/lib/queries/auth.ts. Framework-neutral: both web apps run
// these (the React legal reader, the Vue admin pages).
import { api, ensureOk, ProblemError } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";

export {
  invalidateInstanceWrites,
  publicInstanceQuery,
  refreshPublicInstance,
} from "@/lib/queries/instance";

export const adminUsersQuery = queryOptions({
  queryKey: ["admin", "users"] as const,
  queryFn: async () => ensureOk(await api.GET("/api/v1/admin/users")),
  retry: false,
});

export const adminWorkspacesQuery = queryOptions({
  queryKey: ["admin", "workspaces"] as const,
  queryFn: async () => ensureOk(await api.GET("/api/v1/admin/workspaces")),
  retry: false,
});

export const adminSystemQuery = queryOptions({
  queryKey: ["admin", "system"] as const,
  queryFn: async () => ensureOk(await api.GET("/api/v1/admin/system")),
  retry: false,
});

export const adminInstanceSettingsQuery = queryOptions({
  queryKey: ["admin", "instance-settings"] as const,
  queryFn: async () => ensureOk(await api.GET("/api/v1/admin/instance-settings")),
  retry: false,
});

export const adminAuditQuery = queryOptions({
  queryKey: ["admin-audit"] as const,
  queryFn: async () => ensureOk(await api.GET("/api/v1/admin/audit")),
  retry: (failureCount, error) =>
    !(error instanceof ProblemError && error.status === 404) && failureCount < 3,
});

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

/** The published versions of `kind` (the list itself: its one reader needs no more). */
export function legalVersionsQuery(kind: string) {
  return queryOptions({
    queryKey: ["legal", kind, "versions"] as const,
    queryFn: async () =>
      (
        await ensureOk(
          await api.GET("/api/v1/legal/{kind}/versions", {
            params: { path: { kind } },
          }),
        )
      ).versions,
    retry: false,
  });
}
