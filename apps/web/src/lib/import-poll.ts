// Source apps/web/src/lib/import-poll.ts: abortable status polling with a
// fixed try budget. The caller owns the AbortController (cancel button,
// unmount) and decides what a spent budget means.

export const IMPORT_POLL_MS = 1500;
export const IMPORT_POLL_TRIES = 40;

export type ImportJobStatus = "pending" | "running" | "completed" | "failed";

export type ImportPollOutcome =
  | { kind: "completed" }
  | { kind: "failed" }
  | { kind: "cancelled" }
  | { kind: "budget" };

export function isImportActive(status: string): boolean {
  return status === "pending" || status === "running";
}

function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}

/** WHY: abort must not leave a timer that wakes the next poll first. */
export function abortableSleep(ms: number, signal?: AbortSignal): Promise<"ok" | "cancelled"> {
  if (signal?.aborted) return Promise.resolve("cancelled");
  if (ms <= 0) return Promise.resolve("ok");
  return new Promise((resolve) => {
    let settled = false;
    const finish = (kind: "ok" | "cancelled") => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      resolve(kind);
    };
    const onAbort = () => finish("cancelled");
    const timer = setTimeout(() => finish(signal?.aborted ? "cancelled" : "ok"), ms);
    signal?.addEventListener("abort", onAbort);
  });
}

export async function pollImportJob(opts: {
  fetchStatus: (signal?: AbortSignal) => Promise<{ status: string }>;
  signal?: AbortSignal;
  intervalMs: number;
  maxTries: number;
  sleep?: (ms: number, signal?: AbortSignal) => Promise<"ok" | "cancelled">;
}): Promise<ImportPollOutcome> {
  const { fetchStatus, signal, intervalMs, maxTries } = opts;
  const sleep = opts.sleep ?? abortableSleep;
  for (let tryIndex = 0; tryIndex < maxTries; tryIndex += 1) {
    if (signal?.aborted) return { kind: "cancelled" };
    if (tryIndex > 0) {
      const waited = await sleep(intervalMs, signal);
      if (waited === "cancelled" || signal?.aborted) return { kind: "cancelled" };
    }
    let row: { status: string };
    try {
      row = await fetchStatus(signal);
    } catch (err) {
      if (signal?.aborted || isAbortError(err)) return { kind: "cancelled" };
      throw err;
    }
    if (signal?.aborted) return { kind: "cancelled" };
    if (row.status === "completed") return { kind: "completed" };
    if (row.status === "failed") return { kind: "failed" };
  }
  return { kind: "budget" };
}

export async function fetchImportStatus(
  workspaceId: string,
  importJobId: string,
  signal?: AbortSignal,
): Promise<{ status: string }> {
  const response = await fetch(
    `/api/v1/import/${encodeURIComponent(importJobId)}?workspaceId=${encodeURIComponent(workspaceId)}`,
    { credentials: "include", signal },
  );
  if (!response.ok) throw new Error(`import status failed: ${response.status}`);
  const data = (await response.json()) as { status?: unknown };
  if (typeof data.status !== "string") throw new Error("import status missing");
  return { status: data.status };
}
