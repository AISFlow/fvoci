import assert from "node:assert/strict";
import test from "node:test";
import { QueryObserver } from "@tanstack/query-core";
import { QueryClient, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp, effectScope, ref } from "vue";
import { taskMutationErrorMessage } from "@/features/tasks/task-errors";
import { ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { installNodeRelativeRequestShim } from "../../../../test/node-api-fetch";
import { useRescheduleTask, type RescheduleRequest } from "./useRescheduleTask";

const WS = "workspace-a";
const A = "project-a";
const B = "project-b";
const request: RescheduleRequest = {
  id: "task-a",
  item: {
    startDate: "2026-10-01",
    dueDate: "2026-10-02",
    start: "2026-10-01",
    end: "2026-10-02",
    inferred: "none",
  },
  change: { kind: "move", start: "2026-10-02", end: "2026-10-03" },
};

const flush = () => new Promise((resolve) => setImmediate(resolve));
type Patch = { request: Request; resolve: (response: Response) => void };

function firstPatch(patches: readonly Patch[]): Patch {
  const patch = patches[0];
  assert.ok(patch, "the real API client reached the mocked transport");
  return patch;
}

function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } },
  });
  const target = ref({ workspaceId: WS, projectId: A, timeZone: "UTC" });
  const app = createApp({ render: () => null });
  app.use(VueQueryPlugin, { queryClient: client });
  const scope = effectScope();
  const result = app.runWithContext(() => scope.run(() => useRescheduleTask(() => target.value)));
  assert.ok(result);
  const fetches: string[] = [];
  const unsubscribes = [A, B].map((projectId) =>
    new QueryObserver(client, {
      queryKey: ["task-layout", WS, projectId, "month"],
      initialData: { projectId, revision: 0 },
      queryFn: () => {
        fetches.push(projectId);
        return Promise.resolve({ projectId, revision: 1 });
      },
    }).subscribe(() => {}),
  );
  const originalFetch = globalThis.fetch;
  const restore = installNodeRelativeRequestShim();
  const patches: Patch[] = [];
  globalThis.fetch = (input: RequestInfo | URL) => {
    assert.ok(input instanceof Request);
    assert.equal(input.method, "PATCH");
    return new Promise<Response>((resolve) => {
      patches.push({ request: input, resolve });
    });
  };
  return {
    client,
    target,
    result,
    fetches,
    patches,
    unmountA: () => unsubscribes[0]?.(),
    stop: () => {
      scope.stop();
      unsubscribes.forEach((unsubscribe) => {
        unsubscribe();
      });
      client.clear();
      patches.forEach((patch) => {
        refuse(patch);
      });
      globalThis.fetch = originalFetch;
      restore();
    },
  };
}

function refuse(
  patch: { resolve: (response: Response) => void },
  status = 409,
  code = "document_version_mismatch",
) {
  patch.resolve(Response.json({ type: "about:blank", title: code, status, code }, { status }));
}

await test("late A failure recovers A's layout and never invalidates B or presents A's error there", async () => {
  const { result, target, patches, fetches, client, stop } = setup();
  try {
    result.reschedule(request);
    await flush();
    assert.equal(patches.length, 1);
    assert.equal(patches[0]?.request.url, `http://fvoci.test/api/v1/workspaces/${WS}/tasks/task-a`);
    assert.equal(result.savingId.value, "task-a");
    assert.ok(result.pending.value);
    target.value = { ...target.value, projectId: B };
    await flush();
    refuse(firstPatch(patches));
    await flush();
    assert.deepEqual(fetches, [A], "failure must recover the request's original layout");
    assert.equal(
      client.getQueryData<{ revision: number }>(["task-layout", WS, B, "month"])?.revision,
      0,
    );
    assert.equal(result.error.value, null, "old route failure cannot appear on B");
    assert.equal(result.savingId.value, null);
    assert.equal(result.pending.value, null);
  } finally {
    stop();
  }
});

await test("route change immediately hides A's optimistic bar and pending state", async () => {
  const { result, target, patches, stop } = setup();
  try {
    result.reschedule(request);
    await flush();
    assert.ok(result.pending.value);
    target.value = { ...target.value, projectId: B };
    assert.equal(result.savingId.value, null);
    assert.equal(result.pending.value, null);
    refuse(firstPatch(patches));
    await flush();
  } finally {
    stop();
  }
});

await test("reschedule captures workspace and dates before the asynchronous mutation starts", async () => {
  const { result, target, patches, fetches, stop } = setup();
  try {
    result.reschedule(request);
    target.value = { workspaceId: "workspace-b", projectId: B, timeZone: "Asia/Seoul" };
    await flush();
    assert.equal(patches[0]?.request.url, `http://fvoci.test/api/v1/workspaces/${WS}/tasks/task-a`);
    assert.deepEqual(await firstPatch(patches).request.clone().json(), {
      startDate: "2026-10-02",
      dueDate: "2026-10-03",
      expectedDates: { startDate: "2026-10-01", dueDate: "2026-10-02", dueAt: null },
    });
    refuse(firstPatch(patches));
    await flush();
    assert.deepEqual(fetches, [A]);
    assert.equal(result.error.value, null);
  } finally {
    stop();
  }
});

for (const [status, code, recovery] of [
  [409, "document_version_mismatch", "layout"],
  [400, "dependency_contradiction", "none"],
  [401, "unauthorized", "session"],
] as const) {
  await test(`current project ${code} preserves its error and ${recovery} recovery contract`, async () => {
    const { result, client, patches, fetches, stop } = setup();
    client.setQueryData(meQuery.queryKey, {});
    try {
      result.reschedule(request);
      await flush();
      refuse(firstPatch(patches), status, code);
      await flush();
      assert.equal(
        result.error.value,
        taskMutationErrorMessage(new ProblemError(status, code), "gantt.bar.failed"),
      );
      assert.deepEqual(fetches, recovery === "layout" ? [A] : []);
      assert.equal(client.getQueryState(meQuery.queryKey)?.isInvalidated, recovery === "session");
      assert.equal(result.savingId.value, null);
      assert.equal(result.pending.value, null);
      result.dismissError();
      assert.equal(result.error.value, null);
    } finally {
      stop();
    }
  });
}

await test("A to B to A never revives an earlier route's error", async () => {
  const { result, target, patches, fetches, stop } = setup();
  try {
    result.reschedule(request);
    await flush();
    target.value = { ...target.value, projectId: B };
    target.value = { ...target.value, projectId: A };
    refuse(firstPatch(patches));
    await flush();
    assert.deepEqual(fetches, [A], "the old target still recovers even after leaving its route");
    assert.equal(result.error.value, null);
  } finally {
    stop();
  }
});

await test("late A failure cannot clear or fail B's new reschedule", async () => {
  const { result, target, patches, fetches, stop } = setup();
  try {
    result.reschedule(request);
    await flush();
    target.value = { ...target.value, projectId: B };
    result.reschedule({ ...request, id: "task-b" });
    await flush();
    assert.equal(patches.length, 2);
    assert.equal(result.savingId.value, "task-b");
    refuse(firstPatch(patches));
    await flush();
    assert.deepEqual(fetches, [A]);
    assert.equal(result.error.value, null);
    assert.equal(result.savingId.value, "task-b");
    assert.deepEqual(result.pending.value, {
      id: "task-b",
      start: request.change.start,
      end: request.change.end,
    });
    const second = patches[1];
    assert.ok(second);
    refuse(second);
    await flush();
    assert.deepEqual(fetches, [A, B]);
    assert.equal(
      result.error.value,
      taskMutationErrorMessage(
        new ProblemError(409, "document_version_mismatch"),
        "gantt.bar.failed",
      ),
    );
    assert.equal(result.savingId.value, null);
    assert.equal(result.pending.value, null);
  } finally {
    stop();
  }
});

await test("an existing current-project error disappears when the route target changes", async () => {
  const { result, target, patches, stop } = setup();
  try {
    result.reschedule(request);
    await flush();
    refuse(firstPatch(patches));
    await flush();
    assert.ok(result.error.value);
    target.value = { ...target.value, projectId: B };
    assert.equal(result.error.value, null);
  } finally {
    stop();
  }
});

await test("late failure marks an unmounted A layout stale for return without fetching B", async () => {
  const { result, target, patches, fetches, client, unmountA, stop } = setup();
  try {
    result.reschedule(request);
    await flush();
    target.value = { ...target.value, projectId: B };
    unmountA();
    assert.equal(client.getQueryState(["task-layout", WS, A, "month"])?.isInvalidated, false);
    refuse(firstPatch(patches));
    await flush();
    assert.deepEqual(fetches, []);
    assert.equal(client.getQueryState(["task-layout", WS, A, "month"])?.isInvalidated, true);
    assert.equal(client.getQueryState(["task-layout", WS, B, "month"])?.isInvalidated, false);
    assert.equal(result.error.value, null);
  } finally {
    stop();
  }
});
