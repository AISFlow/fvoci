import { expect, type Page, type Response, test } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "notif-member@example.com",
  password: "memberpass1",
  givenName: "알림",
  familyName: "수신",
};

test("assignment shows unread badge, inbox, and mark-read", async ({ page }) => {
  test.setTimeout(90_000);

  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === owner.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  const membersRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/members`);
  expect(membersRes.ok()).toBe(true);
    let memberId = (await membersRes.json()).items.find(
      (item: { email: string }) => item.email.toLowerCase() === member.email.toLowerCase(),
    )?.userId;
  expect(memberId).toBeTruthy();

  const projRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "NTF", name: "알림", visibility: "workspace" },
  });
  expect(projRes.status()).toBe(201);
  const project = await projRes.json();
  const projectId = project.id;
  const taskRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${projectId}/tasks`,
    { data: { title: "알림 수신 확인 태스크" } },
  );
  expect(taskRes.status()).toBe(201);
  const task = await taskRes.json();
  const assignRes = await page.request.patch(`/api/v1/workspaces/${workspace.id}/tasks/${task.id}`, {
    data: { assigneeIds: [memberId] },
  });
  expect(assignRes.status()).toBe(200);

  const groupRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/groups`, {
    data: { name: "알림팀" },
  });
  expect(groupRes.status(), await groupRes.text()).toBe(201);
  const groupId = (await groupRes.json()).id;
  const addRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/groups/${groupId}/members`,
    { data: { userId: memberId } },
  );
  expect(addRes.status(), await addRes.text()).toBe(201);
  const docCommentRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/projects/${projectId}/documents/${project.rootDocumentId}/comments`,
    { data: { body: "@알림팀 확인", mentionedGroupIds: [groupId] } },
  );
  expect(docCommentRes.status(), await docCommentRes.text()).toBe(201);

  await logout(page);
  await login(page, member.email, member.password);
  // Both notifications (assignment, group mention) are delivered by the
  // outbox relay; wait until both exist before loading any page: the badge and
  // inbox query once on load and the badge refetches only every 30 s.
  await expect
    .poll(
      async () => {
        const res = await page.request.get(
          `/api/v1/workspaces/${workspace.id}/notifications`,
        );
        if (!res.ok()) return false;
        const items: { verb: string }[] = (await res.json()).items;
        return items.some((item) => item.verb.startsWith("comment."));
      },
      { timeout: 15_000 },
    )
    .toBe(true);
  await page.goto(`/w/${owner.workspaceSlug}/wiki`);
  await expect(
    page.getByRole("button", { name: /안 읽은 알림 \d+건/ }),
  ).toBeVisible({ timeout: 15_000 });

  await page.goto(`/w/${owner.workspaceSlug}/notifications`);
  const item = page.getByText(
    `태스크 #${task.number} 「알림 수신 확인 태스크」의 담당자로 지정되었습니다`,
    { exact: true },
  );
  await expect(item).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText("문서에 새 댓글이 달렸습니다", { exact: true })).toBeVisible();
  await item.click();
  await expect(page).toHaveURL(new RegExp(`/w/${owner.workspaceSlug}/[A-Z0-9-]+-\\d+$`));

  await page.goto(`/w/${owner.workspaceSlug}/notifications`);
  const readAll = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/v1/workspaces/${workspace.id}/notifications/read-all`) &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "전체 읽음" }).click();
  expect((await readAll).status()).toBe(200);
  await page.getByRole("tab", { name: "안 읽음" }).click();
  await expect(page.getByText("알림이 없습니다")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "알림", exact: true })).toBeVisible({
    timeout: 15_000,
  });

  await browserPushToggle(page, workspace.id);
});

/**
 * Browser push opt-in through the real UI, `/instance` key, `/sw.js`
 * registration, notification permission and PUT route. Headless Chromium has
 * no push service, so only `PushManager.subscribe/getSubscription` are a
 * controlled in-page boundary (persisted in sessionStorage); nothing reaches
 * FCM or any other push service.
 */
async function browserPushToggle(page: Page, workspaceId: string): Promise<void> {
  await page.context().grantPermissions(["notifications"]);
  await page.addInitScript(() => {
    const KEY = "e2e-push-subscription";
    const LOG = "e2e-push-log";
    const log = (entry: string) => {
      const items: string[] = JSON.parse(sessionStorage.getItem(LOG) ?? "[]");
      items.push(entry);
      sessionStorage.setItem(LOG, JSON.stringify(items));
    };
    const toBase64Url = (bytes: Uint8Array) =>
      btoa(String.fromCharCode(...bytes))
        .replace(/\+/g, "-")
        .replace(/\//g, "_")
        .replace(/=+$/, "");
    const fromBase64Url = (value: string) =>
      Uint8Array.from(atob(value.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
    const build = (stored: { endpoint: string; key: string }) => ({
      endpoint: stored.endpoint,
      expirationTime: null,
      options: { userVisibleOnly: true, applicationServerKey: fromBase64Url(stored.key).buffer },
      toJSON: () => ({
        endpoint: stored.endpoint,
        expirationTime: null,
        keys: {
          p256dh:
            "BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY",
          auth: "EjRWeJCrze8SNFZ4kKvN7w",
        },
      }),
      unsubscribe: async () => {
        if (sessionStorage.getItem("e2e-push-fail-unsubscribe") === "1") {
          sessionStorage.removeItem("e2e-push-fail-unsubscribe");
          log("unsubscribe-failed");
          throw new Error("push service unreachable");
        }
        log("unsubscribe");
        sessionStorage.removeItem(KEY);
        return true;
      },
    });
    PushManager.prototype.getSubscription = async function () {
      const raw = sessionStorage.getItem(KEY);
      return raw ? (build(JSON.parse(raw)) as unknown as PushSubscription) : null;
    };
    PushManager.prototype.subscribe = async function (options?: PushSubscriptionOptionsInit) {
      const key = options?.applicationServerKey;
      if (!(key instanceof Uint8Array)) throw new Error("expected raw applicationServerKey");
      const stored = {
        endpoint: `https://push.e2e.invalid/send/${crypto.randomUUID()}`,
        key: toBase64Url(key),
      };
      log(`subscribe:${stored.key.length}`);
      sessionStorage.setItem(KEY, JSON.stringify(stored));
      return build(stored) as unknown as PushSubscription;
    };
  });

  const instance = await (await page.request.get("/api/v1/instance")).json();
  const publicKey: string | null = instance.values.webPushPublicKey;
  expect(publicKey, "server bootstraps VAPID with ENCRYPTION_KEYS").toMatch(
    /^B[A-Za-z0-9_-]{86}$/,
  );

  const pushLog = async (): Promise<string[]> =>
    JSON.parse((await page.evaluate(() => sessionStorage.getItem("e2e-push-log"))) ?? "[]");
  const putPath = `/api/v1/workspaces/${workspaceId}/push-subscriptions`;
  const isPut = (response: Response) =>
    response.url().endsWith(putPath) && response.request().method() === "PUT";

  await page.goto(`/w/${owner.workspaceSlug}/settings`);
  const toggle = page.getByLabel("브라우저 푸시");
  await expect(toggle).toBeEnabled({ timeout: 15_000 });
  await expect(toggle).not.toBeChecked();

  const saved = page.waitForResponse(isPut);
  await toggle.click();
  const put = await saved;
  expect(put.status()).toBe(200);
  const body = put.request().postDataJSON();
  expect(Object.keys(body).sort()).toEqual(["endpoint", "keys"]);
  expect(body.endpoint).toMatch(/^https:\/\/push\.e2e\.invalid\/send\//);
  await expect(toggle).toBeChecked();
  await expect(page.getByText("푸시 알림을 켜지 못했습니다.")).toHaveCount(0);
  expect(
    await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration("/sw.js");
      const worker = registration?.active ?? registration?.waiting ?? registration?.installing;
      return worker?.scriptURL ?? null;
    }),
  ).toMatch(/\/sw\.js$/);

  // A subscription bound to an older VAPID key (after rotate-vapid) is
  // replaced on load and stored again.
  await page.evaluate(() => {
    const stored = JSON.parse(sessionStorage.getItem("e2e-push-subscription") ?? "{}");
    stored.key = `B${"A".repeat(85)}E`;
    sessionStorage.setItem("e2e-push-subscription", JSON.stringify(stored));
  });
  const resaved = page.waitForResponse(isPut);
  await page.reload();
  expect((await resaved).status()).toBe(200);
  await expect(page.getByLabel("브라우저 푸시")).toBeChecked({ timeout: 15_000 });
  expect(await pushLog()).toEqual(["subscribe:87", "unsubscribe", "subscribe:87"]);

  // Disabling only unsubscribes the browser: there is no delete route.
  const requests: string[] = [];
  page.on("request", (request) => {
    if (request.url().endsWith(putPath)) requests.push(request.method());
  });
  await page.getByLabel("브라우저 푸시").click();
  await expect(page.getByLabel("브라우저 푸시")).not.toBeChecked();
  expect(await pushLog()).toEqual(["subscribe:87", "unsubscribe", "subscribe:87", "unsubscribe"]);
  expect(requests).toEqual([]);

  // Logout disconnects this browser: the logout request reports the endpoint
  // (the server drops the row in the logout transaction) even when the
  // browser-side unsubscribe then fails.
  const reenabled = page.waitForResponse(isPut);
  await page.getByLabel("브라우저 푸시").click();
  expect((await reenabled).status()).toBe(200);
  await expect(page.getByLabel("브라우저 푸시")).toBeChecked();
  const endpoint: string = JSON.parse(
    (await page.evaluate(() => sessionStorage.getItem("e2e-push-subscription"))) ?? "{}",
  ).endpoint;
  await page.evaluate(() => sessionStorage.setItem("e2e-push-fail-unsubscribe", "1"));
  const logoutRequest = page.waitForRequest(
    (request) => request.url().endsWith("/api/v1/auth/logout") && request.method() === "POST",
  );
  await logout(page);
  expect((await logoutRequest).postDataJSON()).toEqual({ pushEndpoint: endpoint });
  expect((await pushLog()).at(-1)).toBe("unsubscribe-failed");

  // The next account in this browser profile never sees the leftover
  // subscription as its own: shown off, and removed from the browser.
  await login(page, owner.email, owner.password);
  await page.goto(`/w/${owner.workspaceSlug}/settings`);
  await expect(page.getByLabel("브라우저 푸시")).toBeEnabled({ timeout: 15_000 });
  await expect(page.getByLabel("브라우저 푸시")).not.toBeChecked();
  await expect.poll(async () => (await pushLog()).at(-1)).toBe("unsubscribe");
  expect(await page.evaluate(() => sessionStorage.getItem("e2e-push-subscription"))).toBeNull();
}
