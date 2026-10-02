import { yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { sameValue } from "@fvoci/editor/vue";
import type * as Y from "yjs";
import { api, ensureOk } from "@/lib/api";

export interface ReadonlyBodyScope {
  workspaceId: string;
  targetId: string;
  kind: "document" | "task";
  actorId: string;
  credentialId: string;
  lifetime: number;
  doc: Y.Doc;
  provider: object;
  generation: number;
  connected: boolean;
  synced: boolean;
  pending: boolean;
  allowed: boolean;
}
type ReadBody = (scope: ReadonlyBodyScope) => Promise<unknown>;
async function readBody(scope: ReadonlyBodyScope): Promise<unknown> {
  const response =
    scope.kind === "task"
      ? ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
            params: { path: { workspace_id: scope.workspaceId, task_id: scope.targetId } },
            cache: "no-store",
          }),
        )
      : ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/body", {
            params: { path: { workspace_id: scope.workspaceId, document_id: scope.targetId } },
            cache: "no-store",
          }),
        );
  // Both JSON routes return the stored projection unchanged. Markdown isn't a
  // valid comparison response; no fields are stripped to manufacture equality.
  if (!("contentJson" in response)) throw new Error("committed JSON body unavailable");
  return response.contentJson;
}
function authorized(scope: ReadonlyBodyScope | null): scope is ReadonlyBodyScope {
  return (
    !!scope &&
    scope.allowed &&
    scope.connected &&
    scope.synced &&
    !scope.pending &&
    !!scope.actorId &&
    !!scope.credentialId
  );
}
function sameScope(before: ReadonlyBodyScope, after: ReadonlyBodyScope): boolean {
  return (
    before.workspaceId === after.workspaceId &&
    before.targetId === after.targetId &&
    before.kind === after.kind &&
    before.actorId === after.actorId &&
    before.credentialId === after.credentialId &&
    before.lifetime === after.lifetime &&
    before.doc === after.doc &&
    before.provider === after.provider &&
    before.generation === after.generation
  );
}

/** View-authorized copy proof, separate from the writer's persist ACK/saved flag. */
export function useReadonlyCommittedBody(
  current: () => ReadonlyBodyScope | null,
  read: ReadBody = readBody,
): () => Promise<boolean> {
  return async () => {
    const before = current();
    if (!authorized(before)) return false;
    let updates = 0;
    const changed = () => {
      updates++;
    };
    before.doc.on("update", changed);
    try {
      const live = yDocToTiptapJson(before.doc);
      const committed = await read(before);
      const after = current();
      return (
        updates === 0 &&
        authorized(after) &&
        sameScope(before, after) &&
        sameValue(live, committed) &&
        sameValue(yDocToTiptapJson(after.doc), committed)
      );
    } catch {
      return false;
    } finally {
      before.doc.off("update", changed);
    }
  };
}
