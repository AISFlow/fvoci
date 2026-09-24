import { queryOptions } from "@tanstack/react-query";
import type { paths } from "@/generated/api";

/** Source `routes.workspaces.lookup`: GET `$WS/lookup/:displayId`. */
export const LOOKUP_PATH = "/api/v1/workspaces/{workspace_id}/lookup/{display_id}" as const;

type GeneratedHasLookup = typeof LOOKUP_PATH extends keyof paths ? true : false;

/**
 * Compile latch: assignment is `false` until generated `paths` includes lookup.
 * Adding the path without wiring `api.GET` must fail typecheck here.
 */
export const GENERATED_LOOKUP_READY: GeneratedHasLookup = false;

export function generatedLookupPath(): typeof LOOKUP_PATH | null {
  return GENERATED_LOOKUP_READY ? LOOKUP_PATH : null;
}

/** Source `lookupItemOutput`. Used by selection tests; not a handwritten OpenAPI client. */
export type LookupItem = {
  kind: "document" | "task";
  id: string;
  displayId: string;
  title: string;
  projectId: string | null;
};

/** Source `lookupListOutput`. Empty items is a miss (SPA notFound), not 403. */
export type LookupList = {
  items: LookupItem[];
};

export function pickLookupTask(
  items: readonly LookupItem[],
  displayId: string,
): LookupItem | null {
  const want = displayId.trim().toUpperCase();
  if (want === "") return null;
  for (const item of items) {
    if (item.kind === "task" && item.displayId.toUpperCase() === want) return item;
  }
  return null;
}

export function lookupQuery(workspaceId: string, displayId: string) {
  return queryOptions({
    queryKey: ["lookup", workspaceId, displayId] as const,
    queryFn: async (): Promise<LookupList> => {
      throw new Error("lookup_not_in_generated_contract");
    },
    enabled: false,
    retry: false,
  });
}
