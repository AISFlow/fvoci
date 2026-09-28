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
 * workspace: on 404 the watcher stops reopening and the caller reconciles
 * once. Otherwise no list is fetched while refused; the caller reconciles once
 * when the reopened stream opens, because the server starts it at the current
 * event horizon, past any access event of the refused gap.
 *
 * A 401 does not stop the watcher. Session loss belongs to the app's session
 * handling (a failed `me` query sends the page to /login), and a list reconcile
 * could not help: it would fail with the same 401. The watcher keeps reopening
 * with the pool's backoff, so a transient 401 heals and then reconciles on
 * the reopen instead of leaving the page unwatched until it remounts.
 *
 * A 404 whose reconcile still lists the workspace (a re-add racing the probe)
 * leaves the watcher stopped until the layout remounts.
 */
export function watchWorkspaceAccess(
  workspaceId: string,
  handlers: {
    onAccessChange: () => void;
    onError?: (event: Event) => void;
  },
): WorkspaceAccessSubscription {
  const url = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/access-stream`;
  let stopped = false;
  // Set on the refusal itself, not when the probe answers: the first reopen can
  // open before a slow probe does.
  let refused = false;

  const onOpen = () => {
    if (!refused || stopped) return;
    refused = false;
    handlers.onAccessChange();
  };

  // Handlers given here are removed by `close()`, so a closed lease leaves no
  // listener on a pooled source that other leases keep reopening.
  const source = openSharedEventSource(url, {
    onOpen,
    onError: handlers.onError,
  });

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
    refused = true;
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

/** 404: not a member, or the workspace is deleted. */
async function workspaceAccessGone(workspaceId: string): Promise<boolean> {
  try {
    const { response } = await api.GET("/api/v1/workspaces/{workspace_id}", {
      params: { path: { workspace_id: workspaceId } },
    });
    return response.status === 404;
  } catch {
    // Network failure: keep reopening.
    return false;
  }
}
