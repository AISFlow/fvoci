import assert from "node:assert/strict";
import { test } from "node:test";
import { InfiniteQueryObserver, QueryClient, type InfiniteData } from "@tanstack/query-core";
import { invalidateTaskCaches, invalidateTaskStreamResyncCaches } from "./task-cache.ts";

const WS = "ws-1";
const PROJECT = "project-1";
const KEY = ["tasks", WS, PROJECT, ""] as const;

type Page = { items: string[]; nextCursor: string | null };
type Get = { cursor: string | null; resolve: (page: Page) => void };

/**
 * A mounted project task list whose GETs the test answers by hand, so a
 * "load more" can be held in flight while a stream hint arrives.
 */
function mountTaskList() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const gets: Get[] = [];
  const observer = new InfiniteQueryObserver<
    Page,
    Error,
    InfiniteData<Page>,
    typeof KEY,
    string | null
  >(client, {
    queryKey: KEY,
    queryFn: ({ pageParam }) =>
      new Promise<Page>((resolve) => {
        gets.push({ cursor: pageParam, resolve });
      }),
    initialPageParam: null,
    getNextPageParam: (lastPage) => lastPage.nextCursor,
  });
  const unsubscribe = observer.subscribe(() => {});
  const pages = () => client.getQueryData<InfiniteData<Page>>(KEY)?.pages ?? [];
  return { client, observer, gets, pages, unsubscribe };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

const invalidations = [
  [
    "stream open/reset resync",
    (client: QueryClient) => invalidateTaskStreamResyncCaches(client, WS, PROJECT),
  ],
  ["task hint", (client: QueryClient) => void invalidateTaskCaches(client, WS, PROJECT, "task-1")],
] as const;

for (const [name, invalidate] of invalidations) {
  test(`${name} during "load more" keeps the requested page and refetches every loaded page`, async () => {
    const { client, observer, gets, pages, unsubscribe } = mountTaskList();
    await flush();
    assert.deepEqual(gets.map((get) => get.cursor), [null]);
    gets[0].resolve({ items: ["a"], nextCursor: "c1" });
    await flush();

    void observer.fetchNextPage();
    await flush();
    assert.equal(gets.at(-1)?.cursor, "c1");
    // The EventSource `open` resync (or a task hint) lands while page 2 is in flight.
    invalidate(client);
    await flush();
    gets[1].resolve({ items: ["b"], nextCursor: null });
    await flush();
    assert.deepEqual(
      pages().map((page) => page.items),
      [["a"], ["b"]],
      "the clicked page is not dropped by the invalidation",
    );

    // The hint is still applied: both loaded pages are fetched again.
    assert.deepEqual(gets.slice(2).map((get) => get.cursor), [null]);
    gets[2].resolve({ items: ["a2"], nextCursor: "c1" });
    await flush();
    assert.deepEqual(gets.slice(2).map((get) => get.cursor), [null, "c1"]);
    gets[3].resolve({ items: ["b2"], nextCursor: null });
    await flush();
    assert.deepEqual(pages().map((page) => page.items), [["a2"], ["b2"]]);
    assert.equal(gets.length, 4);
    unsubscribe();
    client.clear();
  });
}
