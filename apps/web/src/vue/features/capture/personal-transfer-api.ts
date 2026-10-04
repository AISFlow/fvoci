import { api, ensureOk } from "@/lib/api";
import type { TransferBody, TransferSelection } from "./personal-transfer-command";
/** Authorized, effect-free disclosure preview; Cancel after it changes nothing. */
export async function previewPersonalTransfer(sourceWorkspaceId: string, body: TransferSelection) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/personal-transfers/preview", {
      params: { path: { workspace_id: sourceWorkspaceId } },
      body,
    }),
  );
}
export async function confirmPersonalTransfer(sourceWorkspaceId: string, body: TransferBody) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/personal-transfers", {
      params: { path: { workspace_id: sourceWorkspaceId } },
      body,
    }),
  );
}
