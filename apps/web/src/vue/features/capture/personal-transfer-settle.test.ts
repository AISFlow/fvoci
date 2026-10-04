import { expect, test } from "bun:test";
import { InfiniteQueryObserver, QueryClient, type InfiniteData } from "@tanstack/query-core";
import { settleExactKeepingLoadMore } from "./personal-transfer-command";

type Page = { rows: string[]; next: number | null };
const KEY = ["workspace-tasks", "source-ws", "open-assigned"] as const;

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}

/**
 * A real QueryClient with an active infinite observer. The server holds the
 * moved row on page 1 until the MOVE commits; a "load more" request sent before
 * that commit answers with the pre-MOVE page after a delay.
 */
async function retainedPagesWithLoadMoreInFlight() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const server = { moved: true, failing: false };
  const gate = deferred();
  let heldPageOne = true;
  const observer = new InfiniteQueryObserver<Page, Error, InfiniteData<Page>, typeof KEY, number>(
    client,
    {
      queryKey: KEY,
      initialPageParam: 0,
      getNextPageParam: (last) => last.next,
      queryFn: async ({ pageParam }) => {
        const moved = server.moved;
        if (server.failing) throw new Error("refetch failed");
        if (pageParam === 1 && heldPageOne) {
          heldPageOne = false;
          await gate.promise;
        }
        return pageParam === 0
          ? { rows: ["kept"], next: 1 }
          : { rows: moved ? ["moved-task"] : ["other"], next: null };
      },
    },
  );
  const unsubscribe = observer.subscribe(() => undefined);
  await observer.refetch();
  const loadMore = observer.fetchNextPage();
  await Promise.resolve();
  const rows = () =>
    (client.getQueryData<InfiniteData<Page>>(KEY)?.pages ?? []).flatMap((page) => page.rows);
  return { client, server, gate, loadMore, rows, stop: unsubscribe };
}

test("a load-more page fetched before the MOVE cannot keep the moved row once settled", async () => {
  const h = await retainedPagesWithLoadMoreInFlight();
  try {
    h.server.moved = false; // the MOVE commits while the load-more is in flight
    const settled = settleExactKeepingLoadMore(h.client, KEY);
    h.gate.resolve();
    await settled;
    await h.loadMore;
    expect(h.rows()).toEqual(["kept", "other"]);
  } finally {
    h.stop();
  }
});

test("control: a single non-cancelling invalidation accepts the stale page", async () => {
  const h = await retainedPagesWithLoadMoreInFlight();
  try {
    h.server.moved = false;
    const settled = h.client.invalidateQueries(
      { queryKey: KEY, exact: true },
      { cancelRefetch: false },
    );
    h.gate.resolve();
    await settled;
    await h.loadMore;
    expect(h.rows()).toContain("moved-task");
  } finally {
    h.stop();
  }
});

test("a real refetch failure rejects instead of reporting the cache settled", async () => {
  const h = await retainedPagesWithLoadMoreInFlight();
  h.gate.resolve();
  await h.loadMore;
  try {
    h.server.moved = false;
    h.server.failing = true;
    let failure: unknown = null;
    await settleExactKeepingLoadMore(h.client, KEY).catch((error: unknown) => {
      failure = error;
    });
    expect(failure).toBeInstanceOf(Error);
  } finally {
    h.stop();
  }
});
