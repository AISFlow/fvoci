import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { workspacesQuery } from "@/lib/queries";
import { watchWorkspaceAccess } from "@/lib/workspace-access-stream";

/**
 * Reconcile workspace membership when the access stream closes. Transport errors
 * are ignored; only a successful workspace list omitting this id navigates home.
 */
export function useWorkspaceAccessWatch(workspaceId: string | undefined) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  useEffect(() => {
    if (!workspaceId) return;
    const sub = watchWorkspaceAccess(workspaceId, {
      onAccessChange: async () => {
        try {
          const list = await queryClient.fetchQuery({
            ...workspacesQuery,
            staleTime: 0,
          });
          const stillMember = list.items.some((ws) => ws.id === workspaceId);
          if (!stillMember) {
            navigate("/?denied=workspace", { replace: true });
          }
        } catch {
          // Transport or list failure alone must not evict the workspace shell.
        }
      },
    });
    return () => sub.close();
  }, [workspaceId, queryClient, navigate]);
}
