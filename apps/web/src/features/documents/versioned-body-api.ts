import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";

export type VersionedBody = components["schemas"]["VersionedBodyResponse"];
export type BodySaveCommand = components["schemas"]["SaveVersionedBodyInput"];
export type BodySaveResult = components["schemas"]["SaveVersionedBodyResponse"];

export async function readVersionedBody(
  workspaceId: string,
  documentId: string,
  signal?: AbortSignal,
) {
  return ensureOk(
    await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body/versioned", {
      signal,
      params: { path: { workspace_id: workspaceId, document_id: documentId } },
    }),
  );
}

export async function saveVersionedBody(
  workspaceId: string,
  documentId: string,
  body: BodySaveCommand,
) {
  return ensureOk(
    await api.PUT("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body/versioned", {
      params: { path: { workspace_id: workspaceId, document_id: documentId } },
      body,
    }),
  );
}
