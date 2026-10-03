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
  return listen(url, handlers, (body) => {
    const { taskId, verb } = body;
    return typeof taskId === "string" && taskId !== "" && typeof verb === "string" && verb !== ""
      ? { taskId, verb }
      : null;
  });
}

/** A hint from the workspace stream, which names the hinted task's project. */
export type WorkspaceTaskStreamHint = TaskStreamHint & { projectId: string };

/** Per pooled workspace URL: how many subscriptions of this tab watch each project. */
const watchersByUrl = new Map<string, Map<string, number>>();

/**
 * One stream for every project of a workspace (C6): per-project streams hold
 * one HTTP/1.1 socket each and starve ordinary requests at six. The server
 * re-checks project access for every hint. Each connection's `open` resyncs
 * this subscription's projects. A project that nobody in the tab watched
 * while the pooled connection was already open may have missed hints, so it
 * is resynced when it joins, as its own fresh per-project connection did.
 */
export function subscribeWorkspaceTaskStream(
  workspaceId: string,
  projectIds: readonly string[],
  handlers: {
    onTask: (hint: WorkspaceTaskStreamHint) => void;
    onResync: (projectIds: readonly string[]) => void;
    onError?: (event: Event) => void;
  },
): TaskStreamSubscription {
  const url = `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/task-stream`;
  const projects = [...new Set(projectIds)];
  const subscription = listen(
    url,
    {
      onTask: handlers.onTask,
      onResync: () => {
        handlers.onResync(projects);
      },
      onError: handlers.onError,
    },
    (body) => {
      const { taskId, verb, projectId } = body;
      return typeof taskId === "string" &&
        taskId !== "" &&
        typeof verb === "string" &&
        verb !== "" &&
        typeof projectId === "string" &&
        projectId !== ""
        ? { taskId, verb, projectId }
        : null;
    },
  );
  let watchers = watchersByUrl.get(url);
  if (!watchers) {
    watchers = new Map();
    watchersByUrl.set(url, watchers);
  }
  const unwatched = projects.filter((project) => !watchers.get(project));
  for (const project of projects) watchers.set(project, (watchers.get(project) ?? 0) + 1);
  if (subscription.readyState === EventSource.OPEN && unwatched.length > 0)
    handlers.onResync(unwatched);

  let closed = false;
  return {
    close: () => {
      if (closed) return;
      closed = true;
      for (const project of projects) {
        const count = (watchers.get(project) ?? 1) - 1;
        if (count > 0) watchers.set(project, count);
        else watchers.delete(project);
      }
      if (watchers.size === 0 && watchersByUrl.get(url) === watchers) watchersByUrl.delete(url);
      subscription.close();
    },
  };
}

/** Test hook: forget workspace watcher counts (with the shared pool reset). */
export function resetWorkspaceTaskStreamWatchersForTests(): void {
  watchersByUrl.clear();
}

function listen<H>(
  url: string,
  handlers: {
    onTask: (hint: H) => void;
    onResync: () => void;
    onError?: (event: Event) => void;
  },
  parse: (body: Record<string, unknown>) => H | null,
): TaskStreamSubscription & { readonly readyState: number } {
  const source = openSharedEventSource(url, {
    onError: handlers.onError,
  });

  const onOpen = (event: Event) => {
    if (event instanceof MessageEvent) handlers.onResync();
  };
  const onTask = (event: MessageEvent<string>) => {
    try {
      const body: unknown = JSON.parse(event.data);
      if (typeof body !== "object" || body === null) return;
      const hint = parse(body as Record<string, unknown>);
      if (hint) handlers.onTask(hint);
    } catch {
      // Ignore malformed hints; next resync or GET restores state.
    }
  };

  source.addEventListener("open", onOpen);
  source.addEventListener("task", onTask as EventListener);

  return {
    get readyState() {
      return source.readyState;
    },
    close: () => {
      source.removeEventListener("open", onOpen);
      source.removeEventListener("task", onTask as EventListener);
      source.close();
    },
  };
}
