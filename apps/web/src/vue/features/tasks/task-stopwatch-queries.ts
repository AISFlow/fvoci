import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { queryOptions } from "@/lib/query-options";
import type { Query, QueryClient } from "@tanstack/query-core";

export type TimerCommand = components["schemas"]["TimerCommandBody"];

export function timerContextChanged(error: unknown): error is ProblemError {
  return (
    error instanceof ProblemError &&
    error.status === 409 &&
    error.reason === "timer_context_changed"
  );
}

export function taskStopwatchQuery(
  actor: string,
  workspaceId: string,
  taskId: string,
  sessionId: string,
) {
  const queryKey = ["task-timer", actor, sessionId, workspaceId, taskId] as const;
  return queryOptions({
    queryKey,
    queryFn: async ({ signal }) =>
      captureTimerRead(queryKey, async () =>
        ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer", {
            params: {
              path: { workspace_id: workspaceId, task_id: taskId },
              query: { expectedActorId: actor, expectedSessionId: sessionId },
            },
            signal,
          }),
        ),
      ),
    enabled: Boolean(actor && sessionId && workspaceId && taskId),
    // Returning to a cached scope rechecks authority before consuming an old denial.
    staleTime: 0,
    retry: false,
    // Covers other tabs/new sessions; DB remains the only run owner. A server
    // hint can also refetch this exact query, never a generic entity cache.
    refetchInterval: 5000,
  });
}

export async function sendTimerCommand(workspaceId: string, taskId: string, body: TimerCommand) {
  return ensureOk(
    await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer", {
      params: { path: { workspace_id: workspaceId, task_id: taskId } },
      body,
    }),
  );
}

export function ownerStopwatchQuery(actor: string, sessionId: string) {
  const queryKey = ["task-timer-owner", actor, sessionId] as const;
  return queryOptions({
    queryKey,
    queryFn: async ({ signal }) =>
      captureTimerRead(queryKey, async () =>
        ensureOk(
          await api.GET("/api/v1/me/task-timer", {
            params: { query: { expectedActorId: actor, expectedSessionId: sessionId } },
            signal,
          }),
        ),
      ),
    enabled: Boolean(actor && sessionId),
    staleTime: 0,
    retry: false,
    refetchInterval: 5000,
  });
}

export class TimerReadFailure extends ProblemError {
  readonly queryKey: readonly string[];

  constructor(error: ProblemError, queryKey: readonly string[]) {
    super(error.status, error.code, undefined, error.reason);
    this.queryKey = Object.freeze([...queryKey]);
  }
}

export async function captureTimerRead<T>(
  queryKey: readonly string[],
  read: () => Promise<T>,
): Promise<T> {
  try {
    return await read();
  } catch (error) {
    if (error instanceof ProblemError) throw new TimerReadFailure(error, queryKey);
    throw error;
  }
}

export type TimerQueryCapture = {
  readonly queryKey: readonly string[];
  readonly query: Query | undefined;
};

export function captureTimerQuery(
  client: QueryClient,
  queryKey: readonly string[],
): TimerQueryCapture {
  return {
    queryKey: Object.freeze([...queryKey]),
    query: client.getQueryCache().find({ queryKey, exact: true }),
  };
}

export function captureTimerDenial(
  client: QueryClient,
  error: unknown,
  queryKey: readonly string[],
  status: string,
  fetchStatus: string,
): TimerQueryCapture | undefined {
  if (
    !(error instanceof TimerReadFailure) ||
    status !== "error" ||
    fetchStatus !== "idle" ||
    error.queryKey.length !== queryKey.length ||
    !error.queryKey.every((part, index) => part === queryKey[index])
  )
    return;
  const capture = captureTimerQuery(client, error.queryKey);
  // Another mounted observer can already have removed this same denied query.
  // Its cached private display still retires; a replacement query never does.
  if (
    capture.query &&
    (capture.query.state.error !== error ||
      capture.query.state.status !== "error" ||
      capture.query.state.fetchStatus !== "idle")
  )
    return;
  return capture;
}

export async function removeCapturedTimerQuery(
  client: QueryClient,
  capture: TimerQueryCapture,
  current: () => boolean,
): Promise<boolean> {
  const sameQuery = () =>
    client.getQueryCache().find({ queryKey: capture.queryKey, exact: true }) === capture.query;
  if (!current() || !sameQuery()) return false;
  if (capture.query) {
    await client.cancelQueries({
      queryKey: capture.queryKey,
      exact: true,
      predicate: (query) => query === capture.query,
    });
  }
  if (!current() || !sameQuery()) return false;
  client.removeQueries({
    queryKey: capture.queryKey,
    exact: true,
    predicate: (query) => query === capture.query,
  });
  return true;
}
