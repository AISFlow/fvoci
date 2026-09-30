import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp, effectScope } from "vue";
import { useProjectRef } from "./useProjectRef.ts";
import { useWikiDocumentRef } from "./useWikiDocumentRef.ts";
import { type SessionEnvironment, useWorkspaceSession } from "./useWorkspaceSession.ts";

// The Vue session composables against a real QueryClient. There is no
// server here: every request the API client makes fails (bun has no page
// origin for its relative URLs), which is how a refetch is made to fail.

function queryClient(): QueryClient {
  return new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
}

/** Runs `use` as a component setup would: inside an app (for inject) and an effect scope. */
function mount<T>(client: QueryClient, use: () => T): { result: T; stop: () => void } {
  const app = createApp({ render: () => null });
  app.use(VueQueryPlugin, { queryClient: client });
  const scope = effectScope();
  const result = app.runWithContext(() => scope.run(use)) as T;
  return {
    result,
    stop: () => {
      scope.stop();
      client.clear();
    },
  };
}

async function until(condition: () => boolean, what: string): Promise<void> {
  for (let i = 0; i < 200; i += 1) {
    if (condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(`timed out waiting for ${what}`);
}

/** Starts a fetch of `queryKey` that never settles, as a slow retry would. */
function hangFetch(client: QueryClient, queryKey: readonly unknown[]): void {
  void client.fetchQuery({ queryKey, queryFn: () => new Promise<never>(() => {}) }).catch(() => undefined);
}

const WORKSPACE = { id: "11111111-1111-7111-8111-111111111111", slug: "acme", name: "Acme" };
const ME = { userId: "22222222-2222-7222-8222-222222222222", timezone: "Asia/Seoul", locale: "ko" };

function seedSession(client: QueryClient, workspaces = [WORKSPACE]): void {
  client.setQueryData(["setup", "status"], { needed: false });
  client.setQueryData(["auth", "me"], ME);
  client.setQueryData(["me", "workspaces"], { items: workspaces });
}

function testEnvironment(redirects: string[]): SessionEnvironment {
  return {
    redirect: (path) => redirects.push(path),
    location: () => ({ pathname: "/w/acme/WIKI-1", search: "", hash: "" }),
    watchAccess: () => ({ close: () => {} }),
  };
}

test("session: stays ready when refetches fail with cached data", async () => {
  const client = queryClient();
  seedSession(client);
  const redirects: string[] = [];
  const { result, stop } = mount(client, () => useWorkspaceSession("acme", testEnvironment(redirects)));
  try {
    assert.equal(result.status.value, "ready");
    await client.refetchQueries({ queryKey: ["me", "workspaces"] });
    await client.refetchQueries({ queryKey: ["auth", "me"] });
    assert.equal(client.getQueryState(["me", "workspaces"])?.status, "error");
    assert.equal(client.getQueryState(["auth", "me"])?.status, "error");
    assert.equal(result.status.value, "ready");
    assert.equal(result.workspace.value?.id, WORKSPACE.id);
    assert.deepEqual(redirects, []);
  } finally {
    stop();
  }
});

test("session: an error only when a failed query has nothing to show", async () => {
  const client = queryClient();
  client.setQueryData(["setup", "status"], { needed: false });
  client.setQueryData(["me", "workspaces"], { items: [WORKSPACE] });
  const redirects: string[] = [];
  const { result, stop } = mount(client, () => useWorkspaceSession("acme", testEnvironment(redirects)));
  try {
    await until(() => client.getQueryState(["auth", "me"])?.status === "error", "the me request to fail");
    await until(() => result.status.value === "error", "the error status");
    // A transport failure is not a sign-out: no login redirect.
    assert.deepEqual(redirects, []);
  } finally {
    stop();
  }
});

test("session: a failed me refetch plus a fresh list without the workspace redirects", async () => {
  const client = queryClient();
  seedSession(client);
  const redirects: string[] = [];
  const { result, stop } = mount(client, () => useWorkspaceSession("acme", testEnvironment(redirects)));
  try {
    await client.refetchQueries({ queryKey: ["auth", "me"] });
    assert.equal(client.getQueryState(["auth", "me"])?.status, "error");
    client.setQueryData(["me", "workspaces"], { items: [] });
    await until(() => redirects.length > 0, "the denied redirect");
    assert.deepEqual(redirects, ["/?denied=workspace"]);
    assert.equal(result.status.value, "loading", "leaving the page, not an error");
  } finally {
    stop();
  }
});

const PROJECT = { id: "33333333-3333-7333-8333-333333333333", key: "GNT", name: "Gantt" };

test("project ref: stays on the project when a refetch fails with cached data", async () => {
  const client = queryClient();
  client.setQueryData(["projects", WORKSPACE.id], { items: [PROJECT] });
  const { result, stop } = mount(client, () => useProjectRef(WORKSPACE.id, "GNT"));
  try {
    assert.equal(result.project.value?.id, PROJECT.id);
    await result.retry();
    assert.equal(result.projects.isError.value, true);
    assert.equal(result.project.value?.id, PROJECT.id);
    assert.equal(result.failed.value, false);
    assert.equal(result.notFound.value, false);
  } finally {
    stop();
  }
});

test("project ref: an error only without data", async () => {
  const client = queryClient();
  const { result, stop } = mount(client, () => useProjectRef(WORKSPACE.id, "GNT"));
  try {
    await until(() => result.projects.isError.value, "the list to fail");
    assert.equal(result.failed.value, true);
    assert.equal(result.notFound.value, false);
  } finally {
    stop();
  }
});

test("project ref: a failed refetch after not found offers a retry", async () => {
  const client = queryClient();
  client.setQueryData(["projects", WORKSPACE.id], { items: [] });
  const { result, stop } = mount(client, () => useProjectRef(WORKSPACE.id, "GNT"));
  try {
    assert.equal(result.notFound.value, true);
    await result.retry();
    assert.equal(result.projects.isError.value, true);
    assert.equal(result.notFound.value, false);
    assert.equal(result.failed.value, true, "a retry instead of loading forever");
    // The retry keeps the cached list and the error status while it fetches.
    hangFetch(client, ["projects", WORKSPACE.id]);
    await until(() => result.projects.isFetching.value, "the retry to start");
    assert.equal(result.projects.isError.value, true);
    assert.equal(result.failed.value, false, "loading while the retry fetches");
    await client.cancelQueries({ queryKey: ["projects", WORKSPACE.id] });
  } finally {
    stop();
  }
});

const TREE_NODE = {
  id: "44444444-4444-7444-8444-444444444444",
  projectId: null,
  number: 7,
  title: "문서",
  path: "a",
  status: "published",
};

test("wiki document ref: stays on the document when a refetch fails with cached data", async () => {
  const client = queryClient();
  client.setQueryData(["tree", WORKSPACE.id], { items: [TREE_NODE] });
  const { result, stop } = mount(client, () => useWikiDocumentRef(WORKSPACE.id, "WIKI-7"));
  try {
    assert.equal(result.node.value?.id, TREE_NODE.id);
    await result.retry();
    assert.equal(result.tree.isError.value, true);
    assert.equal(result.node.value?.id, TREE_NODE.id);
    assert.equal(result.failed.value, false);
    assert.equal(result.notFound.value, false);
  } finally {
    stop();
  }
});

test("wiki document ref: an error only without data", async () => {
  const client = queryClient();
  const { result, stop } = mount(client, () => useWikiDocumentRef(WORKSPACE.id, "WIKI-7"));
  try {
    await until(() => result.tree.isError.value, "the tree to fail");
    assert.equal(result.failed.value, true);
    assert.equal(result.notFound.value, false);
  } finally {
    stop();
  }
});

test("wiki document ref: not found, then a failed refetch offers a retry", async () => {
  const client = queryClient();
  client.setQueryData(["tree", WORKSPACE.id], { items: [{ ...TREE_NODE, number: 8 }] });
  const { result, stop } = mount(client, () => useWikiDocumentRef(WORKSPACE.id, "WIKI-7"));
  try {
    assert.equal(result.notFound.value, true);
    await result.retry();
    assert.equal(result.notFound.value, false);
    assert.equal(result.failed.value, true);
    hangFetch(client, ["tree", WORKSPACE.id]);
    await until(() => result.tree.isFetching.value, "the retry to start");
    assert.equal(result.tree.isError.value, true);
    assert.equal(result.failed.value, false, "loading while the retry fetches");
    await client.cancelQueries({ queryKey: ["tree", WORKSPACE.id] });
  } finally {
    stop();
  }
});

test("wiki document ref: a project document or a malformed ref is not a wiki document", () => {
  const client = queryClient();
  client.setQueryData(["tree", WORKSPACE.id], {
    items: [{ ...TREE_NODE, projectId: PROJECT.id }],
  });
  const project = mount(client, () => useWikiDocumentRef(WORKSPACE.id, "WIKI-7"));
  const malformed = mount(queryClient(), () => useWikiDocumentRef(WORKSPACE.id, "WIKI-07"));
  try {
    assert.equal(project.result.notFound.value, true);
    assert.equal(malformed.result.notFound.value, true);
  } finally {
    project.stop();
    malformed.stop();
  }
});
