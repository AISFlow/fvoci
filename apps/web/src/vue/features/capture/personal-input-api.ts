import { api, ensureOk } from "@/lib/api";
import type { PersonalInputBody } from "./capture-command";
export async function ensurePersonalWorkspace() {
  return ensureOk(await api.POST("/api/v1/me/personal-workspace"));
}
export async function createPersonalInput(workspaceId: string, body: PersonalInputBody) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/personal-input", {
      params: { path: { workspace_id: workspaceId } },
      body,
    }),
  );
}
