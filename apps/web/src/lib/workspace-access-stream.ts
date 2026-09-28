import { api } from "@/lib/api";
import { openSharedEventSource } from "@/lib/shared-event-source";

export type WorkspaceAccessSubscription = {
  close: () => void;
};

/**
 * Workspace access signal: the server closes the stream when membership or session
 * access may have changed. Callers must reconcile via a fresh workspace list; only
 * a successful list missing the workspace should evict. Transport errors alone
 * must not evict (handled in the layout watcher).
 *
 * When the server ends the stream the browser reconnects by itself, and the
 * caller reconciles. A refused connection (401, 404, 429 at the stream cap, a
 * proxy's 5xx during a restart) is reopened by the pool with capped backoff.
 * Each refusal first asks the server whether this session still has the
 * workspace: on 401 or 404 the watcher stops reopening and the caller
 * reconciles once; otherwise it waits for the reopen without a list refetch.
 */
export function watchWorkspaceAccess(
  workspaceId: string,
  handlers: {
    onAccessChange: () => void;
    onError?: (event: Event) => void;
  },
): WorkspaceAccessSubscription {
  const url = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/access-stream`;
  const source = openSharedEventSource(url, {
    onError: handlers.onError,
  });
  let stopped = false;

  const stop = () => {
    if (stopped) return;
    stopped = true;
    source.removeEventListener("error", onClose);
    source.close();
  };

  const onClose = () => {
    if (source.readyState !== EventSource.CLOSED) {
      handlers.onAccessChange();
      return;
    }
    void workspaceAccessGone(workspaceId).then((gone) => {
      if (!gone || stopped) return;
      stop();
      handlers.onAccessChange();
    });
  };

  source.addEventListener("error", onClose);

  return {
    close: stop,
  };
}

/** 401 (session gone) or 404 (not a member, or the workspace is deleted). */
async function workspaceAccessGone(workspaceId: string): Promise<boolean> {
  try {
    const { response } = await api.GET("/api/v1/workspaces/{workspace_id}", {
      params: { path: { workspace_id: workspaceId } },
    });
    return response.status === 401 || response.status === 404;
  } catch {
    // Network failure: keep reopening.
    return false;
  }
}
