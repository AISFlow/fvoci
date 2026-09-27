import type { QueryClient } from "@tanstack/react-query";

export async function invalidateTaskCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
  taskId: string,
): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["task", workspaceId, taskId] }),
    queryClient.invalidateQueries({ queryKey: ["task-activity", workspaceId, taskId] }),
    queryClient.invalidateQueries({ queryKey: ["task-layout", workspaceId, projectId] }),
    queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] }),
    queryClient.invalidateQueries({ queryKey: ["project-collection", workspaceId, projectId] }),
    queryClient.invalidateQueries({ queryKey: ["collection", workspaceId] }),
    queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] }),
  ]);
}

export function invalidateTaskStreamResyncCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
): void {
  void queryClient.invalidateQueries({ queryKey: ["task-layout", workspaceId, projectId] });
  void queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] });
  void queryClient.invalidateQueries({ queryKey: ["project-collection", workspaceId, projectId] });
  void queryClient.invalidateQueries({ queryKey: ["collection", workspaceId] });
  void queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] });
}
