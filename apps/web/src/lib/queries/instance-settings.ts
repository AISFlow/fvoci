// Adapted from source apps/web/src/lib/queries/auth.ts `aiEnabledQueryOptions`.
import { queryOptions } from "@/lib/query-options";
import { publicInstanceQuery, selectAiEnabled } from "@/lib/queries/instance";

export { selectAiEnabled };

/**
 * The AI menu reads one clause of the public `/instance` view under the shared `["instance"]`
 * key, so the document screen adds no round trip and the admin settings save
 * (`invalidateInstanceWrites`) refreshes it.
 */
export const aiEnabledQuery = queryOptions({
  ...publicInstanceQuery,
  select: selectAiEnabled,
});
