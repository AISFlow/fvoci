import { queryOptions } from "@tanstack/react-query";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import type { ViewQuery } from "@/lib/view-query";

type Schemas = components["schemas"];
export type DocumentTag = Schemas["DocumentTagOutput"];
export type DocumentTagPoolItem = Schemas["DocumentTagPoolItemOutput"];
export type DocumentTagPool = Schemas["DocumentTagPoolListResponse"];
export type CollectionField = Schemas["CollectionFieldOutput"];
export type CollectionOption = Schemas["CollectionOptionOutput"];
export type CollectionView = Schemas["CollectionViewOutput"];
export type CollectionViewList = Schemas["CollectionViewListResponse"];
export type CollectionQueryResponse = Schemas["CollectionQueryResponse"];
export type CollectionQueryItem = Schemas["CollectionQueryItemOutput"];
export type CollectionQueryPreview = Schemas["CollectionQueryPreviewOutput"];
export type ProjectCollection = Schemas["ProjectCollectionOutput"];
export type ProjectView = Schemas["ProjectViewOutput"];

/** Source label color keys (`labelColorKey`). */
export const TAG_COLORS = [
  "gray",
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "violet",
  "pink",
] as const;
export type TagColor = (typeof TAG_COLORS)[number];

/**
 * The generated schema types JSON configs/values as `Record<string, never>`;
 * this is the one place that widens our typed objects into that shape.
 */
export function asJsonObject(value: object): Record<string, never> {
  return value as unknown as Record<string, never>;
}

export type CollectionConfig = {
  query: ViewQuery;
  groupBy: string | null;
  dateBy: string | null;
};

export type CollectionQueryBody = {
  config: CollectionConfig;
  limit?: number;
  cursor?: string;
  group?: string | null;
  day?: string | null;
  window?: { from: string; to: string; timeZone: string };
};

export function documentTagPoolQuery(workspaceId: string, q = "") {
  return queryOptions({
    queryKey: ["document-tags", workspaceId, "pool", q] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/document-tags", {
          params: {
            path: { workspace_id: workspaceId },
            query: { limit: 100, ...(q.trim() ? { q: q.trim() } : {}) },
          },
        }),
      ),
    enabled: Boolean(workspaceId),
    retry: false,
  });
}

export function documentAssignedTagsQuery(
  workspaceId: string,
  documentId: string,
  projectId: string | null,
) {
  return queryOptions({
    queryKey: ["document-tags", workspaceId, "assigned", documentId] as const,
    queryFn: async () => {
      const result = projectId
        ? await api.GET(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags",
            {
              params: {
                path: {
                  workspace_id: workspaceId,
                  project_id: projectId,
                  document_id: documentId,
                },
              },
            },
          )
        : await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags", {
            params: { path: { workspace_id: workspaceId, document_id: documentId } },
          });
      return (await ensureOk(result)).items;
    },
    enabled: Boolean(workspaceId) && Boolean(documentId),
    retry: false,
  });
}

export function projectCollectionQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["project-collection", workspaceId, projectId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/collection", {
          params: { path: { workspace_id: workspaceId, project_id: projectId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export function collectionPrefix(workspaceId: string, collectionId: string) {
  return ["collection", workspaceId, collectionId] as const;
}

export function collectionFieldsQuery(workspaceId: string, collectionId: string) {
  return queryOptions({
    queryKey: [...collectionPrefix(workspaceId, collectionId), "fields"] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields", {
          params: { path: { workspace_id: workspaceId, collection_id: collectionId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(collectionId),
    retry: false,
  });
}

export function collectionViewsQuery(workspaceId: string, collectionId: string) {
  return queryOptions({
    queryKey: [...collectionPrefix(workspaceId, collectionId), "views"] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views", {
          params: { path: { workspace_id: workspaceId, collection_id: collectionId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(collectionId),
    retry: false,
  });
}

export function collectionRowsQuery(
  workspaceId: string,
  collectionId: string,
  body: CollectionQueryBody,
  enabled = true,
) {
  return queryOptions({
    queryKey: [...collectionPrefix(workspaceId, collectionId), "query", body] as const,
    queryFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/query", {
          params: { path: { workspace_id: workspaceId, collection_id: collectionId } },
          body: { ...body, config: asJsonObject(body.config) },
        }),
      ),
    enabled: enabled && Boolean(workspaceId) && Boolean(collectionId),
    retry: false,
  });
}

export function taskCollectionItemQuery(workspaceId: string, taskId: string) {
  return queryOptions({
    queryKey: ["collection-item", workspaceId, "task", taskId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/collection-item", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(taskId),
    retry: false,
  });
}

export function projectViewsQuery(workspaceId: string, projectId: string) {
  return queryOptions({
    queryKey: ["project-views", workspaceId, projectId] as const,
    queryFn: async () =>
      (
        await ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/views", {
            params: { path: { workspace_id: workspaceId, project_id: projectId } },
          }),
        )
      ).items,
    enabled: Boolean(workspaceId) && Boolean(projectId),
    retry: false,
  });
}

export async function putCollectionValue(
  workspaceId: string,
  collectionId: string,
  itemId: string,
  body: {
    fieldId: string;
    expectedVersion: number;
    expectedFieldVersion: number;
    value: object | null;
  },
) {
  return ensureOk(
    await api.PUT(
      "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/items/{item_id}/values",
      {
        params: {
          path: { workspace_id: workspaceId, collection_id: collectionId, item_id: itemId },
        },
        body: { ...body, value: body.value === null ? null : asJsonObject(body.value) },
      },
    ),
  );
}
