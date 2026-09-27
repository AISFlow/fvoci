import { openSharedEventSource } from "@/lib/shared-event-source";

export type WorkspaceAccessSubscription = {
  close: () => void;
};

/**
 * Workspace access signal: the server closes the stream when membership or session
 * access may have changed. Callers must reconcile via a fresh workspace list; only
 * a successful list missing the workspace should evict. Transport errors alone
 * must not evict (handled in the layout watcher).
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

  const onClose = () => {
    handlers.onAccessChange();
  };

  source.addEventListener("error", onClose);

  return {
    close: () => {
      source.removeEventListener("error", onClose);
      source.close();
    },
  };
}
