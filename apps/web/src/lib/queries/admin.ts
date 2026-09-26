// Adapted from source apps/web/src/lib/queries/admin.ts and the legal queries
// in apps/web/src/lib/queries/auth.ts.
import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, ensureOk, ProblemError } from "@/lib/api";

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

export const publicInstanceQuery = queryOptions({
  queryKey: ["instance"] as const,
  queryFn: async () => ensureOk(await api.GET("/api/v1/instance")),
});

/**
 * Revalidates `/instance` past the browser's 60 s HTTP cache (ETag, so a 304
 * when nothing changed) and stores it under the shared `["instance"]` key.
 * For screens that act on a policy an admin may just have changed.
 */
export function refreshPublicInstance(queryClient: QueryClient) {
  return queryClient.fetchQuery({
    queryKey: publicInstanceQuery.queryKey,
    queryFn: async () => ensureOk(await api.GET("/api/v1/instance", { cache: "no-cache" })),
    staleTime: 0,
  });
}

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
    select: (data) => data.versions,
    retry: false,
  });
}

/** Operator text and attachment preview mode also live in the public `/instance` view. */
export async function invalidateInstanceWrites(queryClient: QueryClient): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["admin", "instance-settings"] }),
    queryClient.invalidateQueries({ queryKey: ["instance"] }),
    queryClient.invalidateQueries({ queryKey: ["setup", "status"] }),
  ]);
}
