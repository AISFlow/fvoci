import assert from "node:assert/strict";
import { test } from "node:test";
import {
  InfiniteQueryObserver,
  QueryClient,
  QueryObserver,
  type InfiniteData,
} from "@tanstack/query-core";
import { invalidateTaskCaches, invalidateTaskStreamResyncCaches } from "./task-cache.ts";

const WS = "ws-1";
const PROJECT = "project-1";
// The project task list and one collection board column (#142): both page with
// "load more" and both are refreshed by the project task stream.
const LISTS = [
  ["task list", ["tasks", WS, PROJECT, ""]],
  [
    "collection board column",
    ["collection", WS, "collection-1", "board", { groupBy: "status" }, "s1"],
  ],
] as const;

type Page = { items: string[]; nextCursor: string | null };
type Get = { cursor: string | null; resolve: (page: Page) => void };

/**
 * A mounted paged list whose GETs the test answers by hand, so a
 * "load more" can be held in flight while a stream hint arrives.
 */
function mountList(key: readonly unknown[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const gets: Get[] = [];
  const observer = new InfiniteQueryObserver<
    Page,
    Error,
    InfiniteData<Page>,
    readonly unknown[],
    string | null
  >(client, {
    queryKey: key,
    queryFn: ({ pageParam }) =>
      new Promise<Page>((resolve) => {
        gets.push({ cursor: pageParam, resolve });
      }),
    initialPageParam: null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
  });
  const unsubscribe = observer.subscribe(() => {});
  const pages = () => client.getQueryData<InfiniteData<Page>>(key)?.pages ?? [];
  return { client, observer, gets, pages, unsubscribe };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

const invalidations = [
  [
    "stream open resync",
    (client: QueryClient) => {
      invalidateTaskStreamResyncCaches(client, WS, PROJECT);
    },
  ],
  ["task hint", (client: QueryClient) => invalidateTaskCaches(client, WS, PROJECT, "task-1")],
] as const;

for (const [list, key] of LISTS) {
  for (const [name, invalidate] of invalidations) {
    await test(`${list}: ${name} during "load more" keeps the requested page and refetches every loaded page`, async () => {
      const { client, observer, gets, pages, unsubscribe } = mountList(key);
      await flush();
      assert.deepEqual(
        gets.map((get) => get.cursor),
        [null],
      );
      gets[0].resolve({ items: ["a"], nextCursor: "c1" });
      await flush();

      const nextPage = observer.fetchNextPage();
      await flush();
      assert.equal(gets.at(-1)?.cursor, "c1");
      // The EventSource `open` resync (or a task hint) lands while page 2 is in flight.
      const invalidation = invalidate(client);
      await flush();
      gets[1].resolve({ items: ["b"], nextCursor: null });
      await flush();
      assert.deepEqual(
        pages().map((page) => page.items),
        [["a"], ["b"]],
        "the clicked page is not dropped by the invalidation",
      );

      // The hint is still applied: both loaded pages are fetched again.
      assert.deepEqual(
        gets.slice(2).map((get) => get.cursor),
        [null],
      );
      gets[2].resolve({ items: ["a2"], nextCursor: "c1" });
      await flush();
      assert.deepEqual(
        gets.slice(2).map((get) => get.cursor),
        [null, "c1"],
      );
      gets[3].resolve({ items: ["b2"], nextCursor: null });
      await flush();
      assert.deepEqual(
        pages().map((page) => page.items),
        [["a2"], ["b2"]],
      );
      assert.equal(gets.length, 4);
      await Promise.all([nextPage, invalidation]);
      unsubscribe();
      client.clear();
    });
  }
}

await test("task hints invalidate captured workspace/project caches without touching another workspace", async () => {
  const client = new QueryClient();
  const affected = [
    ["task", WS, "task-1"],
    ["task-activity", WS, "task-1"],
    ["task-time-entries", WS, "task-1"],
    ["collection-item", WS, "task", "task-1"],
    ["task-layout", WS, PROJECT, "month"],
    ["tasks", WS, PROJECT, "filter"],
    ["project-collection", WS, PROJECT],
    ["collection", WS, "c1"],
    ["projects", WS],
  ];
  const unaffected = [
    ["task", "other-ws", "task-1"],
    ["tasks", WS, "other-project"],
    ["task-layout", WS, "other-project"],
    ["collection", "other-ws", "c1"],
    ["task", WS, "other-task"],
    ["task-time-entries", WS, "other-task"],
    ["task-time-entries", "other-ws", "task-1"],
    ["collection-item", WS, "task", "other-task"],
    ["collection-item", "other-ws", "task", "task-1"],
    ["collection-item", WS, "document", "task-1"],
    ["auth", "me"],
  ];
  for (const key of [...affected, ...unaffected]) client.setQueryData(key, {});
  await invalidateTaskCaches(client, WS, PROJECT, "task-1");
  for (const key of affected) assert.equal(client.getQueryState(key)?.isInvalidated, true);
  for (const key of unaffected) assert.equal(client.getQueryState(key)?.isInvalidated, false);
  client.clear();
});

await test("authorized resync invalidates retained project detail/activity but preserves sibling scope", async () => {
  const client = new QueryClient();
  const detail = ["task", WS, "task-1"];
  const activity = ["task-activity", WS, "task-1"];
  const sibling = ["task", WS, "task-2"];
  const siblingActivity = ["task-activity", WS, "task-2"];
  const foreign = ["task", "other-ws", "task-1"];
  client.setQueryData(detail, { id: "task-1", projectId: PROJECT });
  client.setQueryData(sibling, { id: "task-2", projectId: "other-project" });
  client.setQueryData(foreign, { id: "task-1", projectId: PROJECT });
  client.setQueryData(activity, {});
  client.setQueryData(siblingActivity, {});
  const grants = [
    ["task-time-entries", WS, "task-1"],
    ["collection-item", WS, "task", "task-1"],
  ];
  const siblingGrants = [
    ["task-time-entries", WS, "task-2"],
    ["task-time-entries", "other-ws", "task-1"],
    ["collection-item", WS, "task", "task-2"],
    ["collection-item", "other-ws", "task", "task-1"],
    ["collection-item", WS, "document", "task-1"],
  ];
  for (const key of [...grants, ...siblingGrants]) client.setQueryData(key, {});
  invalidateTaskStreamResyncCaches(client, WS, PROJECT);
  await flush();
  for (const key of [detail, activity, ...grants])
    assert.equal(client.getQueryState(key)?.isInvalidated, true);
  for (const key of [sibling, siblingActivity, foreign, ...siblingGrants])
    assert.equal(client.getQueryState(key)?.isInvalidated, false);
  client.clear();
});

for (const [name, invalidate] of invalidations) {
  await test(`${name} refetches mounted REST grants from the server without promoting sibling grants`, async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { staleTime: Infinity, retry: false } },
    });
    client.setQueryData(["task", WS, "task-1"], { id: "task-1", projectId: PROJECT });
    client.setQueryData(["task", WS, "task-2"], { id: "task-2", projectId: "other-project" });
    const queries = [
      { key: ["task-time-entries", WS, "task-1"], grant: "canCreate", refetch: true },
      { key: ["collection-item", WS, "task", "task-1"], grant: "canEdit", refetch: true },
      { key: ["task-time-entries", WS, "task-2"], grant: "canCreate", refetch: false },
      { key: ["collection-item", WS, "task", "task-2"], grant: "canEdit", refetch: false },
      { key: ["task-time-entries", "other-ws", "task-1"], grant: "canCreate", refetch: false },
      { key: ["collection-item", "other-ws", "task", "task-1"], grant: "canEdit", refetch: false },
    ];
    const gets: string[][] = [];
    const unsubscribes = queries.map(({ key, grant }) =>
      new QueryObserver(client, {
        queryKey: key,
        initialData: { [grant]: false },
        queryFn: () => {
          gets.push(key);
          return Promise.resolve({ [grant]: true });
        },
      }).subscribe(() => {}),
    );
    try {
      assert.equal(gets.length, 0);
      await invalidate(client);
      await flush();
      for (const { key, grant, refetch } of queries) {
        assert.equal(client.getQueryData<Record<string, boolean>>(key)?.[grant], refetch);
      }
      assert.deepEqual(
        gets,
        queries.filter((query) => query.refetch).map((query) => query.key),
      );
    } finally {
      unsubscribes.forEach((unsubscribe) => {
        unsubscribe();
      });
      client.clear();
    }
  });
}
