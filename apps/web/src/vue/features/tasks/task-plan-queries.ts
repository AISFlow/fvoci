import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";
import { captureTimerRead } from "./task-stopwatch-queries";

export type PlanTaskCommand = components["schemas"]["StudyPlanTaskBody"];
export function planTargetsQuery(
  actor: string,
  session: string,
  workspace: string,
  document: string,
) {
  const queryKey = ["task-plan-targets", actor, session, workspace, document] as const;
  return queryOptions({
    queryKey,
    queryFn: ({ signal }) =>
      captureTimerRead(queryKey, async () =>
        ensureOk(
          await api.GET(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/study-plan/task",
            {
              params: {
                path: { workspace_id: workspace, document_id: document },
                query: { expectedActorId: actor, expectedSessionId: session },
              },
              signal,
            },
          ),
        ),
      ),
    enabled: Boolean(actor && session && workspace && document),
    retry: false,
    staleTime: 0,
    refetchInterval: 5000,
  });
}
export async function sendPlanTask(workspace: string, document: string, body: PlanTaskCommand) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/study-plan/task", {
      params: { path: { workspace_id: workspace, document_id: document } },
      body,
    }),
  );
}
