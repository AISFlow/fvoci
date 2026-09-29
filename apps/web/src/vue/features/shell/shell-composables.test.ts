import assert from "node:assert/strict";
import test from "node:test";
import { t } from "@fvoci/i18n";
import { QueryClient, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp, effectScope, ref } from "vue";
import type { NotificationItem } from "@/features/notifications/notification-target";
import { useLogout, type LogoutEnvironment } from "./useLogout.ts";
import { useNotificationBell } from "./useNotificationBell.ts";
import { useDebouncedTrim, useSearchPalette, useSearchShortcut } from "./useSearchPalette.ts";

// The Vue shell's composables. Queries run against a real QueryClient with
// no server: every request the API client makes fails here (bun has no page
// origin for its relative URLs), which is how a write is made to fail.

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

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// ---- logout ----

function logoutEnvironment(answer: () => Promise<{ response: { ok: boolean; status: number } }>) {
  const redirects: string[] = [];
  const env: LogoutEnvironment = { request: answer, redirect: (path) => redirects.push(path) };
  return { env, redirects };
}

test("logout: a signed-out session leaves for the login page", async () => {
  const { env, redirects } = logoutEnvironment(async () => ({ response: { ok: true, status: 200 } }));
  const { error, logout } = useLogout(env);
  await logout();
  assert.equal(error.value, null);
  assert.deepEqual(redirects, ["/login"]);
});

test("logout: a transport failure stays on the page with the network message", async () => {
  const { env, redirects } = logoutEnvironment(async () => {
    throw new TypeError("Failed to fetch");
  });
  const { error, logout } = useLogout(env);
  await logout();
  assert.equal(error.value, t("error.network"));
  assert.deepEqual(redirects, []);
});

test("logout: a refused logout says so, and a retry that succeeds clears it", async () => {
  let ok = false;
  const { env, redirects } = logoutEnvironment(async () => ({ response: { ok, status: ok ? 200 : 500 } }));
  const { error, logout } = useLogout(env);
  await logout();
  assert.equal(error.value, t("error.auth.logout"));
  assert.deepEqual(redirects, []);
  ok = true;
  const retry = logout();
  assert.equal(error.value, null, "the old message goes while the retry runs");
  await retry;
  assert.deepEqual(redirects, ["/login"]);
});

// ---- search palette ----

type Key = Event & { key: string; ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean };

function key(init: { key: string; ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean }): Key {
  return Object.assign(new Event("keydown", { cancelable: true }), init) as Key;
}

test("search shortcut: Ctrl+K or Cmd+K opens, Escape closes an open palette", () => {
  const target = new EventTarget();
  const open = ref(false);
  const scope = effectScope();
  scope.run(() => useSearchShortcut(open, target));
  try {
    const plain = key({ key: "k" });
    target.dispatchEvent(plain);
    assert.equal(open.value, false);
    assert.equal(plain.defaultPrevented, false);

    const escapeClosed = key({ key: "Escape" });
    target.dispatchEvent(escapeClosed);
    assert.equal(escapeClosed.defaultPrevented, false, "Escape is left alone while the palette is closed");

    const alreadyHandled = key({ key: "k", ctrlKey: true });
    alreadyHandled.preventDefault();
    target.dispatchEvent(alreadyHandled);
    assert.equal(open.value, false, "an already-handled Mod-K is left alone");

    const ctrl = key({ key: "k", ctrlKey: true });
    target.dispatchEvent(ctrl);
    assert.equal(open.value, true);
    assert.equal(ctrl.defaultPrevented, true, "the browser's own Ctrl+K does not run");

    const escape = key({ key: "Escape" });
    target.dispatchEvent(escape);
    assert.equal(open.value, false);
    assert.equal(escape.defaultPrevented, true);

    target.dispatchEvent(key({ key: "K", metaKey: true, shiftKey: true }));
    assert.equal(open.value, true, "Cmd+Shift+K too");
  } finally {
    scope.stop();
  }
  open.value = false;
  target.dispatchEvent(key({ key: "k", ctrlKey: true }));
  assert.equal(open.value, false, "the listener goes with the scope");
});

test("search palette: the query settles, trimmed, once typing pauses", async () => {
  const source = ref("");
  const scope = effectScope();
  const settled = scope.run(() => useDebouncedTrim(source, 30))!;
  try {
    source.value = "문";
    await sleep(10);
    source.value = " 문서 ";
    await sleep(10);
    assert.equal(settled.value, "", "still typing");
    await sleep(40);
    assert.equal(settled.value, "문서");
  } finally {
    scope.stop();
  }
});

const WORKSPACE_ID = "11111111-1111-7111-8111-111111111111";

test("search palette: searches every kind in hybrid mode, and nothing for an empty query", async () => {
  const client = queryClient();
  const { result, stop } = mount(client, () =>
    useSearchPalette({ workspaceId: WORKSPACE_ID, keyTarget: new EventTarget(), debounceMs: 10 }),
  );
  try {
    assert.equal(result.results.fetchStatus.value, "idle");
    result.draft.value = "  계획  ";
    await until(() => result.q.value === "계획", "the query to settle");
    const key = ["search", WORKSPACE_ID, "계획", "all", "", "", "hybrid"];
    await until(() => client.getQueryCache().find({ queryKey: key, exact: true }) !== undefined, "the search");
    await until(() => result.results.isError.value, "the search to fail without a server");
    assert.deepEqual(result.items.value, []);
  } finally {
    stop();
  }
});

// ---- notification bell ----

const SLUG = "acme";

function notification(overrides: Partial<NotificationItem>): NotificationItem {
  return {
    actorFamilyName: null,
    actorGivenName: null,
    actorUserId: null,
    archivedAt: null,
    createdAt: "2031-03-01T00:00:00Z",
    displayId: "NTF-1",
    eventId: "55555555-5555-7555-8555-555555555555",
    id: "66666666-6666-7666-8666-666666666666",
    payload: { number: 1, title: "알림" },
    readAt: null,
    targetId: null,
    targetType: null,
    verb: "task.updated",
    workspaceId: WORKSPACE_ID,
    ...overrides,
  };
}

function mountBell(client: QueryClient) {
  const navigations: string[] = [];
  const mounted = mount(client, () =>
    useNotificationBell({ workspaceId: WORKSPACE_ID, slug: SLUG, navigate: (path) => navigations.push(path) }),
  );
  return { ...mounted, navigations };
}

test("bell: named by the unread count, with a capped badge", async () => {
  const client = queryClient();
  client.setQueryData(["notifications-unread", WORKSPACE_ID], { count: 3 });
  const { result, stop } = mountBell(client);
  try {
    assert.equal(result.label.value, t("notif.unreadCount", { count: 3 }));
    assert.equal(result.badge.value, "3");
    client.setQueryData(["notifications-unread", WORKSPACE_ID], { count: 120 });
    await until(() => result.count.value === 120, "the new count");
    assert.equal(result.badge.value, "99+");
    client.setQueryData(["notifications-unread", WORKSPACE_ID], { count: 0 });
    await until(() => result.count.value === 0, "no unread");
    assert.equal(result.label.value, t("notif.bell.title"));
  } finally {
    stop();
  }
});

test("bell: the list loads only while the panel is open", async () => {
  const client = queryClient();
  client.setQueryData(["notifications-unread", WORKSPACE_ID], { count: 1 });
  const { result, stop } = mountBell(client);
  try {
    await sleep(20);
    assert.equal(result.list.fetchStatus.value, "idle");
    assert.equal(result.list.isPending.value, true);
    result.open.value = true;
    await until(() => result.list.isError.value, "the list request (it fails without a server)");
  } finally {
    stop();
  }
});

test("bell: a read notification closes the panel and opens its item", async () => {
  const client = queryClient();
  client.setQueryData(["notifications-unread", WORKSPACE_ID], { count: 0 });
  const { result, stop, navigations } = mountBell(client);
  try {
    result.open.value = true;
    await result.openItem(notification({ readAt: "2031-03-02T00:00:00Z", displayId: "WIKI-4" }));
    assert.equal(result.open.value, false);
    assert.deepEqual(navigations, ["/w/acme/WIKI-4"]);
    // Nothing to open: the panel still closes.
    result.open.value = true;
    await result.openItem(notification({ readAt: "2031-03-02T00:00:00Z", displayId: null }));
    assert.equal(result.open.value, false);
    assert.deepEqual(navigations, ["/w/acme/WIKI-4"]);
  } finally {
    stop();
  }
});

test("bell: an unread notification is marked read first; a failed write goes nowhere", async () => {
  const client = queryClient();
  client.setQueryData(["notifications-unread", WORKSPACE_ID], { count: 1 });
  const { result, stop, navigations } = mountBell(client);
  try {
    result.open.value = true;
    await assert.rejects(result.openItem(notification({})));
    assert.equal(result.open.value, true);
    assert.deepEqual(navigations, []);
    await assert.rejects(result.readAll());
  } finally {
    stop();
  }
});

test("bell: an item reads as its notification message", () => {
  const client = queryClient();
  const { result, stop } = mountBell(client);
  try {
    // An assignment arrives as task.updated without a status move.
    assert.equal(
      result.message(notification({ verb: "task.updated", payload: { number: 7, title: "배치" } })),
      "태스크 #7 「배치」의 담당자로 지정되었습니다",
    );
    assert.equal(
      result.message(notification({ verb: "task.updated", payload: "not an object" })),
      t("notif.task.assignee"),
    );
  } finally {
    stop();
  }
});
