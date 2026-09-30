import type { QueryClient } from "@tanstack/query-core";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";

// The public `/instance` view, framework-neutral: both web apps read it
// under the shared ["instance"] key (lib/queries/admin.ts re-exports these
// for the React admin pages).

export type PublicInstance = components["schemas"]["InstanceSettingsOutput"];

export const publicInstanceQuery = queryOptions({
  queryKey: ["instance"] as const,
  // Server sends Cache-Control max-age=60 + ETag; after admin updates, the browser
  // can reuse a pre-patch empty body within that window (see service-info-flow e2e).
  queryFn: async () => ensureOk(await api.GET("/api/v1/instance", { cache: "no-cache" })),
});

/**
 * Revalidates `/instance` past the browser's 60 s HTTP cache (ETag, so a 304
 * when nothing changed) and stores it under the shared `["instance"]` key.
 * For screens that act on a policy an admin may just have changed.
 */
export function refreshPublicInstance(queryClient: QueryClient) {
  return queryClient.query({
    queryKey: publicInstanceQuery.queryKey,
    queryFn: async () => ensureOk(await api.GET("/api/v1/instance", { cache: "no-cache" })),
    staleTime: 0,
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

/** Only an explicit `true` opens the AI menu; this is a UI gate, the AI routes still enforce access. */
export function selectAiEnabled(data: PublicInstance): boolean {
  // HTTP payloads can be missing or malformed despite the generated wire type.
  const ai: unknown = data.values.features.ai;
  return ai === true;
}
