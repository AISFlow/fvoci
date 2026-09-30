import { openSharedEventSource } from "@/lib/shared-event-source";

export type TaskStreamHint = {
  verb: string;
  taskId: string;
};

export type TaskStreamSubscription = {
  close: () => void;
};

/**
 * Project task invalidation stream, shared by the React and Vue hosts
 * (hooks/use-task-stream.ts, vue/composables/useTaskStream.ts).
 *
 * The server sends two events (src/http/routes/streams.rs
 * queue_item_to_event): `open`, first on every connection once the stream is
 * authorized, and `task` hints. The server's `open` triggers `onResync`, once
 * per connection: on the first connect, on the browser's own retry after a
 * 200 stream ended, and on the pool's reopen after a refused connection, so
 * hints missed in between are recovered. The browser also dispatches its
 * native `open` (the 200 response) to the same "open" listeners; that one is
 * not a MessageEvent and is ignored, or every connect would resync twice.
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

  const onOpen = (event: Event) => {
    if (event instanceof MessageEvent) handlers.onResync();
  };
  const onTask = (event: MessageEvent<string>) => {
    try {
      const body = JSON.parse(event.data) as Partial<TaskStreamHint> | null;
      if (body?.taskId && body?.verb) {
        handlers.onTask(body);
      }
    } catch {
      // Ignore malformed hints; next resync or GET restores state.
    }
  };

  source.addEventListener("open", onOpen);
  source.addEventListener("task", onTask as EventListener);

  return {
    close: () => {
      source.removeEventListener("open", onOpen);
      source.removeEventListener("task", onTask as EventListener);
      source.close();
    },
  };
}
