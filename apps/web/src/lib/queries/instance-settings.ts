// Adapted from source apps/web/src/lib/queries/auth.ts `aiEnabledQueryOptions`.
import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { publicInstanceQuery } from "@/lib/queries/admin";

type PublicInstance = components["schemas"]["InstanceSettingsOutput"];

/** Only an explicit `true` opens the AI menu; this is a UI gate, the AI routes still enforce access. */
export function selectAiEnabled(data: PublicInstance): boolean {
  return data.values.features.ai === true;
}

/**
 * The AI menu reads one clause of the public `/instance` view under the shared `["instance"]`
 * key, so the document screen adds no round trip and the admin settings save
 * (`invalidateInstanceWrites`) refreshes it.
 */
export const aiEnabledQuery = queryOptions({
  ...publicInstanceQuery,
  select: selectAiEnabled,
});
