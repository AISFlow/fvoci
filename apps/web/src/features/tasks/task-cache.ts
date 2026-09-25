import type { QueryClient } from "@tanstack/react-query";

export async function invalidateTaskCaches(
  queryClient: QueryClient,
  workspaceId: string,
  projectId: string,
  taskId: string,
): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["task", workspaceId, taskId] }),
    queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId, projectId] }),
    queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] }),
  ]);
}
