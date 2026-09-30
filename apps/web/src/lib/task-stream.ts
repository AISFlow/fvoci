import { openSharedEventSource } from "@/lib/shared-event-source";

export type TaskStreamHint = {
  verb: string;
  taskId: string;
};

export type TaskStreamSubscription = {
  close: () => void;
};

/**
 * Project task invalidation stream. `open` / `reset` events trigger `onResync`,
 * including the `open` of a source the pool reopened after a refused
 * connection, which recovers hints missed in between.
 * `task` events carry invalidation hints only (never authoritative ACL).
 */
export function subscribeTaskStream(
  workspaceId: string,
  projectId: string,
  handlers: {
    onTask: (hint: TaskStreamHint) => void;
    onResync: () => void;
    onError?: (event: Event) => void;
  },
): TaskStreamSubscription {
  const url = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}/stream`;
  const source = openSharedEventSource(url, {
    onError: handlers.onError,
  });

  const onOpen = () => {
    handlers.onResync();
  };
  const onReset = () => {
    handlers.onResync();
  };
  const onTask = (event: MessageEvent<string>) => {
    try {
      const body = JSON.parse(event.data) as TaskStreamHint;
      if (body?.taskId && body?.verb) {
        handlers.onTask(body);
      }
    } catch {
      // Ignore malformed hints; next resync or GET restores state.
    }
  };

  source.addEventListener("open", onOpen);
  source.addEventListener("reset", onReset);
  source.addEventListener("task", onTask as EventListener);

  return {
    close: () => {
      source.removeEventListener("open", onOpen);
      source.removeEventListener("reset", onReset);
      source.removeEventListener("task", onTask as EventListener);
      source.close();
    },
  };
}
