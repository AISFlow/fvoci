import { api, ensureOk } from "@/lib/api";
import type { components } from "@/generated/api";
import { queryOptions } from "@/lib/query-options";

export type ConnectInput = components["schemas"]["ConnectBody"];
export type LinkInput = components["schemas"]["LinkBody"];
export type Reference = components["schemas"]["ReferenceOutput"];
export type Library = components["schemas"]["LibraryOutput"];
export interface ActorScope {
  userId: string;
  sessionId: string;
}
export function actorKey(actor: ActorScope) {
  return ["zotero", actor.userId, actor.sessionId] as const;
}
export function workspacesQuery(actor: ActorScope) {
  return queryOptions({
    queryKey: [...actorKey(actor), "workspaces"],
    retry: false,
    queryFn: async ({ signal }) => ensureOk(await api.GET("/api/v1/me/workspaces", { signal })),
  });
}
export function connectorsQuery(actor: ActorScope, workspaceId: string) {
  return queryOptions({
    queryKey: [...actorKey(actor), workspaceId, "connectors"],
    retry: false,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/zotero", {
          params: { path: { workspace_id: workspaceId } },
          signal,
        }),
      ),
  });
}
export function libraryQuery(actor: ActorScope, workspaceId: string, connectorId: string) {
  return queryOptions({
    queryKey: [...actorKey(actor), workspaceId, "library", connectorId],
    retry: false,
    queryFn: async ({ signal }) =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}", {
          params: { path: { workspace_id: workspaceId, connector_id: connectorId } },
          signal,
        }),
      ),
  });
}
// Keep the write-only key out of Vue Query's mutation-variable cache.
export async function connect(workspaceId: string, input: ConnectInput) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/zotero", {
      params: { path: { workspace_id: workspaceId } },
      body: input,
    }),
  );
}
export async function ensurePersonalWorkspace() {
  return ensureOk(await api.POST("/api/v1/me/personal-workspace"));
}
export async function sync(workspaceId: string, connectorId: string) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}/sync", {
      params: { path: { workspace_id: workspaceId, connector_id: connectorId } },
    }),
  );
}
export async function disconnect(workspaceId: string, connectorId: string) {
  return ensureOk(
    await api.DELETE("/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}", {
      params: { path: { workspace_id: workspaceId, connector_id: connectorId } },
    }),
  );
}
export async function link(workspaceId: string, referenceId: string, input: LinkInput) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/zotero/references/{reference_id}/links", {
      params: { path: { workspace_id: workspaceId, reference_id: referenceId } },
      body: input,
    }),
  );
}

export async function lookupTarget(workspaceId: string, displayId: string, kind: string) {
  const result = await ensureOk(
    await api.GET("/api/v1/workspaces/{workspace_id}/lookup/{display_id}", {
      params: { path: { workspace_id: workspaceId, display_id: displayId.trim().toUpperCase() } },
    }),
  );
  const target = result.items.find((item) => item.kind === kind);
  if (!target) throw new Error("authorized target not found");
  return target;
}
