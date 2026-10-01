import type { QueryClient } from "@tanstack/vue-query";
import type { Router } from "vue-router";

/** Available only in the separately compiled task-cache-e2e fixture. */
export function installCacheObserver(client: QueryClient, router: Router): void {
  const adapter = {
    push: (path: string) => router.push(path),
    discoverySnapshot: (ws: string, tag = "") => {
      const state = client.getQueryState(["wiki-discovery", ws, tag]);
      return {
        staleTime: client.getDefaultOptions().queries?.staleTime,
        age: Date.now() - (state?.dataUpdatedAt ?? 0),
        updatedAt: state?.dataUpdatedAt,
        invalidated: state?.isInvalidated,
        countsUpdatedAt: client.getQueryState(["me", "workspaces"])?.dataUpdatedAt,
        workspaceCount: client
          .getQueryData<{ items: { id: string; documentCount: number }[] }>(["me", "workspaces"])
          ?.items.find((item) => item.id === ws)?.documentCount,
      };
    },
  };
  Object.defineProperty(window, "fvociCacheObservationFixture", { value: adapter });
}

export type CacheObservationWindow = Window & {
  fvociCacheObservationFixture: {
    push: (path: string) => Promise<unknown>;
    discoverySnapshot: (
      ws: string,
      tag?: string,
    ) => {
      staleTime: unknown;
      age: number;
      updatedAt: number | undefined;
      invalidated: boolean | undefined;
      countsUpdatedAt: number | undefined;
      workspaceCount: number | undefined;
    };
  };
};
