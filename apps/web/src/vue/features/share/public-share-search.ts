import { api, ensureOk, ProblemError } from "@/lib/api";
import type { components } from "@/generated/api";
import { queryOptions } from "@/lib/query-options";

export type ShareSearchItem = components["schemas"]["SearchItemOutput"];

/** The anonymous index is recall only; Rust rechecks each hit against PostgreSQL. */
export function publicShareSearchQuery(token: string, q: string) {
  return queryOptions({
    queryKey: ["share-search", token, q] as const,
    staleTime: 0,
    enabled: Boolean(token) && q.trim().length > 0 && q.length <= 200,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/share/{token}/search", {
          params: { path: { token }, query: { q } },
          signal,
        }),
      ),
    retry: (count, error) =>
      !(error instanceof ProblemError && [404, 429].includes(error.status)) && count < 2,
  });
}
