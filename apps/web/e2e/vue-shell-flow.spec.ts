// The Vue app's workspace shell (the header and footer around the Vue pages:
// the project Gantt and wiki documents) against the React shell's contract:
// logout, the notification bell, the search palette, the legal footer, the
// workspace switch and the push session rebind. Each control is exercised on
// both Vue pages. Runs against the production build the Rust server serves,
// with the real PostgreSQL, Meilisearch and outbox of the e2e group.
import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type BrowserContext,
  type Page,
  type Request,
  type Response,
} from "@playwright/test";
import { createE2eUser, login, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "vshell",
  workspaceName: "Vue Shell E2E",
};

const member = {
  email: "vue-shell-member@example.com",
  password: "memberpass1",
  givenName: "셸",
  familyName: "동료",
};

// A fixed month, so the Gantt bars do not depend on the day the suite runs.
const GANTT_QUERY = "y=2031&m=3";
const PROJECT_KEY = "VSH";

// Icon sets must be bundled; these are the Iconify API hosts a runtime fetch would hit.
const ICON_API_HOSTS = ["api.iconify.design", "api.simplesvg.com", "api.unisvg.com"];

function watchIconRequests(page: Page): string[] {
  const hits: string[] = [];
  page.on("request", (request) => {
    const host = new URL(request.url()).hostname;
    if (ICON_API_HOSTS.includes(host)) hits.push(request.url());
  });
  return hits;
}

/** CSP reports and Iconify fetches of one page; both must stay empty. */
function watchPage(page: Page): { csp: string[]; icons: string[] } {
  return { csp: watchCspViolations(page), icons: watchIconRequests(page) };
}

async function ensureSetup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(admin.familyName);
    await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
    await page.getByLabel("이메일").fill(admin.email);
    await page.getByLabel("비밀번호").fill(admin.password);
    await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
    await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
    return;
  }
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
}

async function workspaceId(request: APIRequestContext): Promise<string> {
  const res = await request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = (await res.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  return workspace.id;
}

type Project = { id: string; key: string };
type Task = { id: string; number: number; title: string };
type WikiDoc = { id: string; number: number; title: string; path: string };

/** The Gantt project (created once; later tests find it). */
async function ganttProject(request: APIRequestContext, wsId: string): Promise<Project> {
  const list = await request.get(`/api/v1/workspaces/${wsId}/projects`);
  expect(list.ok()).toBe(true);
  const existing = (await list.json()).items.find((item: Project) => item.key === PROJECT_KEY);
  if (existing) return existing;
  const res = await request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: PROJECT_KEY, name: "셸 간트", visibility: "workspace" },
  });
  expect(res.status(), await res.text()).toBe(201);
  const project = (await res.json()) as Project;
  // A bar in the Gantt's month, so the chart (not its empty state) shows.
  await createTask(request, wsId, project.id, { title: "셸 막대", startDate: "2031-03-03", dueDate: "2031-03-05" });
  return project;
}

async function createTask(
  request: APIRequestContext,
  wsId: string,
  projectId: string,
  data: Record<string, unknown>,
): Promise<Task> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, { data });
  expect(res.status(), await res.text()).toBe(201);
  return res.json();
}

async function createDoc(request: APIRequestContext, wsId: string, title: string): Promise<WikiDoc> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/documents`, { data: { parentId: null, title } });
  expect(res.status(), await res.text()).toBe(201);
  const doc = (await res.json()) as { id: string; number: number };
  return { ...doc, title, path: `/w/${admin.workspaceSlug}/WIKI-${doc.number}` };
}

const ganttPath = () => `/w/${admin.workspaceSlug}/${PROJECT_KEY}/gantt?${GANTT_QUERY}`;

async function openGantt(page: Page): Promise<void> {
  const navigation = await page.goto(ganttPath());
  expect(navigation?.status()).toBe(200);
  await expect(page.locator('[data-slot="gantt"]')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
}

async function openWiki(page: Page, doc: WikiDoc): Promise<void> {
  const navigation = await page.goto(doc.path);
  expect(navigation?.status()).toBe(200);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
}

/** A marker on this document: gone after a full page load, kept by an in-app navigation. */
async function markDocument(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as { __sameDocument?: boolean }).__sameDocument = true;
  });
}

async function sameDocument(page: Page): Promise<boolean> {
  return (await page.evaluate(() => (window as unknown as { __sameDocument?: boolean }).__sameDocument)) === true;
}

async function newSignedInPage(
  browser: Browser,
  baseURL: string | undefined,
  who: { email: string; password: string },
): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  await login(page, who.email, who.password);
  return { context, page };
}

type Opener = { name: string; open: (page: Page) => Promise<void> };

test("the footer's service information and policy links load their public Vue pages from both workspace pages", async ({
  page,
}) => {
  await ensureSetup(page);
  const seen = watchPage(page);
  const wsId = await workspaceId(page.request);
  await ganttProject(page.request, wsId);
  const doc = await createDoc(page.request, wsId, "셸 바닥글");
  const pages: Opener[] = [
    { name: "gantt", open: openGantt },
    { name: "wiki", open: (p) => openWiki(p, doc) },
  ];
  const targets = [
    { link: "서비스 정보", path: "/service-info", shows: page.getByRole("heading", { name: "서비스 정보" }) },
    { link: "이용약관", path: "/legal/terms", shows: page.getByText("문서가 없습니다") },
    { link: "개인정보처리방침", path: "/legal/privacy", shows: page.getByText("문서가 없습니다") },
  ];

  for (const vuePage of pages) {
    await vuePage.open(page);
    const footer = page.locator("footer").getByRole("navigation", { name: "서비스 정보" });
    for (const target of targets) {
      await expect(footer.getByRole("link", { name: target.link, exact: true }), vuePage.name).toHaveAttribute(
        "href",
        target.path,
      );
    }
    for (const target of targets) {
      await vuePage.open(page);
      await markDocument(page);
      await page.locator("footer").getByRole("link", { name: target.link, exact: true }).click();
      await expect(page).toHaveURL(new RegExp(`${target.path}$`));
      await expect(target.shows).toBeVisible();
      await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
      expect(await sameDocument(page), `${vuePage.name} → ${target.path} is a full load`).toBe(false);
    }
  }
  expect(seen.csp).toEqual([]);
  expect(seen.icons).toEqual([]);
});

test("the search palette finds seeded documents and tasks and opens them from both Vue pages", async ({ page }) => {
  test.setTimeout(120_000);
  await ensureSetup(page);
  const seen = watchPage(page);
  const wsId = await workspaceId(page.request);
  const project = await ganttProject(page.request, wsId);
  const token = `vshell${Date.now()}`;
  const first = await createDoc(page.request, wsId, `${token} 첫 문서`);
  const second = await createDoc(page.request, wsId, `${token} 둘째 문서`);
  const task = await createTask(page.request, wsId, project.id, { title: `${token} 태스크` });
  const taskRef = `${PROJECT_KEY}-${task.number}`;

  // Meilisearch indexes through the outbox; wait until all three are found.
  await expect
    .poll(
      async () => {
        const res = await page.request.get(
          `/api/v1/workspaces/${wsId}/search?q=${encodeURIComponent(token)}&type=all`,
        );
        if (!res.ok()) return [];
        return ((await res.json()).items as { title: string }[]).map((item) => item.title).sort();
      },
      { timeout: 30_000 },
    )
    .toEqual([task.title, first.title, second.title].sort());

  const palette = page.getByRole("dialog", { name: "빠른 검색" });
  const searchRequest = () =>
    page.waitForRequest((request: Request) => {
      const url = new URL(request.url());
      return url.pathname === `/api/v1/workspaces/${wsId}/search` && url.searchParams.get("q") === token;
    });

  // Gantt: Ctrl+K, the same search the React palette makes, then a wiki
  // document result: an in-app navigation to the Vue wiki page.
  await openGantt(page);
  await markDocument(page);
  await page.keyboard.press("Control+k");
  await expect(palette).toBeVisible();
  const input = palette.getByLabel("검색어");
  await expect(input).toBeFocused();
  await expect(palette.getByText("문서·태스크·댓글·첨부 파일명을 검색합니다.", { exact: false })).toBeVisible();
  const request = searchRequest();
  await input.fill(token);
  const url = new URL((await request).url());
  expect(url.searchParams.get("mode")).toBe("hybrid");
  expect(url.searchParams.get("type")).toBe("all");
  await expect(palette.getByRole("link", { name: new RegExp(first.title) })).toBeVisible({ timeout: 10_000 });
  await expect(palette.getByRole("link", { name: new RegExp(task.title) })).toBeVisible();
  await expect(palette.getByRole("link", { name: "모든 결과 보기" })).toHaveAttribute(
    "href",
    `/w/${admin.workspaceSlug}/search?q=${token}`,
  );
  await palette.getByRole("link", { name: new RegExp(first.title) }).click();
  await expect(page).toHaveURL(new RegExp(`${first.path}$`));
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(page.getByLabel("문서 제목")).toHaveValue(first.title);
  await expect(palette).toHaveCount(0);
  expect(await sameDocument(page), "Gantt → wiki document stays in the Vue app").toBe(true);

  // Wiki: the button opens it; Escape closes it and focus returns to the button.
  const trigger = page.getByRole("button", { name: "검색", exact: true });
  await trigger.click();
  await expect(palette).toBeVisible();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  await expect(palette.getByLabel("검색어")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(palette).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");

  await trigger.click();
  await expect(palette).toBeVisible();
  await palette.getByRole("button", { name: "검색 닫기" }).click();
  await expect(palette).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");

  // Ctrl+K from inside the editor; another wiki document keeps this page
  // (the shell stays mounted) and the palette, like the bell's open panel,
  // closes with the navigation.
  const bellPanel = page.getByRole("region", { name: "알림" });
  await page.getByRole("button", { name: /^(알림|안 읽은 알림 \d+건)$/ }).click();
  await expect(bellPanel).toBeVisible();
  await page.locator(".fvoci-editor .ProseMirror").click();
  await page.keyboard.press("Control+k");
  await expect(palette.getByLabel("검색어")).toBeFocused();
  await palette.getByLabel("검색어").fill(token);
  await palette.getByRole("link", { name: new RegExp(second.title) }).click();
  await expect(page).toHaveURL(new RegExp(`${second.path}$`));
  await expect(page.getByLabel("문서 제목")).toHaveValue(second.title);
  await expect(palette).toHaveCount(0);
  await expect(bellPanel).toHaveCount(0);
  expect(await sameDocument(page), "wiki → wiki stays in the Vue app").toBe(true);

  // Task results now share the Vue runtime with wiki documents.
  await page.keyboard.press("Control+k");
  await palette.getByLabel("검색어").fill(token);
  const taskLink = palette.getByRole("link", { name: new RegExp(task.title) });
  await expect(taskLink).toHaveAttribute("href", `/w/${admin.workspaceSlug}/${taskRef}`);
  await taskLink.click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${taskRef}$`));
  await expect(page.getByRole("heading", { name: task.title })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await expect(palette).toHaveCount(0);
  expect(await sameDocument(page), "wiki → task stays in the Vue app").toBe(true);

  // Enter opens the search page (React) with the query, from either Vue page;
  // a click on the backdrop closes the palette. A drag that started in the
  // query field does not.
  for (const open of [openGantt, (p: Page) => openWiki(p, first)]) {
    await open(page);
    await page.getByRole("button", { name: "검색", exact: true }).click();
    await expect(palette).toBeVisible();
    const inputBox = await palette.getByLabel("검색어").boundingBox();
    expect(inputBox).toBeTruthy();
    await page.mouse.move(inputBox!.x + inputBox!.width / 2, inputBox!.y + inputBox!.height / 2);
    await page.mouse.down();
    await page.mouse.move(5, 5);
    await page.mouse.up();
    await expect(palette).toBeVisible();
    await page.mouse.click(5, 5);
    await expect(palette).toHaveCount(0);
    await page.keyboard.press("Control+k");
    await palette.getByLabel("검색어").fill(token);
    await markDocument(page);
    await palette.getByLabel("검색어").press("Enter");
    await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/search\\?q=${token}$`));
    await expect(page.getByRole("region", { name: "검색" }).getByText(first.title)).toBeVisible({ timeout: 10_000 });
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    expect(await sameDocument(page), "the search palette opens its Vue page with the existing full navigation").toBe(false);
  }

  // No match.
  await openGantt(page);
  await page.keyboard.press("Control+k");
  await palette.getByLabel("검색어").fill("qxzjvkwpfy");
  await expect(palette.getByText("결과가 없습니다")).toBeVisible({ timeout: 10_000 });
  expect(seen.csp).toEqual([]);
  expect(seen.icons).toEqual([]);
});

test("the bell shows a notification created through the API and opens it from both Vue pages", async ({
  page,
  browser,
  baseURL,
}) => {
  test.setTimeout(120_000);
  await ensureSetup(page);
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
  const wsId = await workspaceId(page.request);
  const project = await ganttProject(page.request, wsId);
  const doc = await createDoc(page.request, wsId, "셸 알림");
  const members = await page.request.get(`/api/v1/workspaces/${wsId}/members`);
  expect(members.ok()).toBe(true);
  const memberId = (await members.json()).items.find(
    (item: { email: string }) => item.email.toLowerCase() === member.email,
  )?.userId;
  expect(memberId).toBeTruthy();

  const m = await newSignedInPage(browser, baseURL, member);
  const seen = watchPage(m.page);
  type Item = {
    id: string;
    verb: string;
    displayId: string | null;
    readAt: string | null;
    payload: { title?: string } | null;
  };
  const notifications = async (): Promise<Item[]> => {
    const res = await m.page.request.get(`/api/v1/workspaces/${wsId}/notifications`);
    expect(res.ok()).toBe(true);
    return (await res.json()).items;
  };
  const unreadCount = async (): Promise<number> => {
    const res = await m.page.request.get(`/api/v1/workspaces/${wsId}/notifications/unread-count`);
    expect(res.ok()).toBe(true);
    return (await res.json()).count;
  };
  /**
   * Assigns a new task to the member and waits until the outbox delivered its
   * notification; the page loads after that (the badge polls every 30 s).
   */
  const assign = async (title: string): Promise<{ task: Task; item: Item }> => {
    const task = await createTask(page.request, wsId, project.id, { title });
    const res = await page.request.patch(`/api/v1/workspaces/${wsId}/tasks/${task.id}`, {
      data: { assigneeIds: [memberId] },
    });
    expect(res.status(), await res.text()).toBe(200);
    let item: Item | undefined;
    await expect
      .poll(
        async () => {
          // The assignment (the member may also hear of the new task itself).
          item = (await notifications()).find(
            (candidate) => candidate.verb === "task.updated" && candidate.payload?.title === title,
          );
          return item !== undefined;
        },
        { timeout: 15_000 },
      )
      .toBe(true);
    return { task, item: item! };
  };

  try {
    // Gantt: the unread count names the bell; opening the notification marks
    // it read and goes to its task in the same Vue runtime.
    const { task: first, item: firstItem } = await assign("셸 알림 태스크 하나");
    const unread = await unreadCount();
    expect(unread).toBeGreaterThan(0);
    await openGantt(m.page);
    const bell = m.page.getByRole("button", { name: `안 읽은 알림 ${unread}건` });
    await expect(bell).toBeVisible({ timeout: 15_000 });
    await expect(bell).toHaveAttribute("aria-expanded", "false");
    await bell.click();
    await expect(bell).toHaveAttribute("aria-expanded", "true");
    const panel = m.page.getByRole("region", { name: "알림" });
    const message = `태스크 #${first.number} 「${first.title}」의 담당자로 지정되었습니다`;
    await expect(panel.getByRole("button", { name: message, exact: true })).toBeVisible();
    await expect(panel.getByRole("link", { name: "모두 보기" })).toHaveAttribute(
      "href",
      `/w/${admin.workspaceSlug}/notifications`,
    );
    await markDocument(m.page);
    const patched = m.page.waitForResponse(
      (response: Response) =>
        /\/notifications\/[0-9a-f-]+$/.test(new URL(response.url()).pathname) &&
        response.request().method() === "PATCH",
    );
    await panel.getByRole("button", { name: message, exact: true }).click();
    expect((await patched).status()).toBe(200);
    await expect(m.page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/${firstItem.displayId}$`));
    await expect(m.page.getByRole("heading", { name: first.title })).toBeVisible();
    await expect(m.page.locator("#root[data-v-app]")).toHaveCount(1);
    await expect(panel).toHaveCount(0);
    expect(await sameDocument(m.page), "notification → task stays in the Vue app").toBe(true);
    expect((await notifications()).find((item) => item.id === firstItem.id)?.readAt).toBeTruthy();

    // Wiki: read all clears the count; "see all" opens the inbox (React).
    const { task: second } = await assign("셸 알림 태스크 둘");
    await openWiki(m.page, doc);
    await m.page.getByRole("button", { name: `안 읽은 알림 ${await unreadCount()}건` }).click();
    const secondMessage = `태스크 #${second.number} 「${second.title}」의 담당자로 지정되었습니다`;
    await expect(panel.getByRole("button", { name: secondMessage, exact: true })).toBeVisible();
    const readAll = m.page.waitForResponse(
      (response: Response) =>
        response.url().endsWith(`/api/v1/workspaces/${wsId}/notifications/read-all`) &&
        response.request().method() === "POST",
    );
    await panel.getByRole("button", { name: "모두 읽음" }).click();
    expect((await readAll).status()).toBe(200);
    await expect(m.page.getByRole("button", { name: "알림", exact: true })).toBeVisible();
    await expect(panel.getByRole("button", { name: "모두 읽음" })).toHaveCount(0);
    expect((await notifications()).every((item) => item.readAt !== null)).toBe(true);
    await markDocument(m.page);
    await panel.getByRole("link", { name: "모두 보기" }).click();
    await expect(m.page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/notifications$`));
    await expect(m.page.getByText(secondMessage, { exact: true }).first()).toBeVisible();
    expect(await sameDocument(m.page)).toBe(false);

    // The toggle closes the panel again. On a 390px viewport the header
    // wraps, so the panel must stay on-screen and below the trigger.
    await m.page.setViewportSize({ width: 390, height: 844 });
    for (const open of [openGantt, (p: Page) => openWiki(p, doc)]) {
      await open(m.page);
      const readBell = m.page.getByRole("button", { name: "알림", exact: true });
      await readBell.click();
      await expect(panel).toBeVisible();
      const panelBox = await panel.boundingBox();
      const bellBox = await readBell.boundingBox();
      expect(panelBox).toBeTruthy();
      expect(bellBox).toBeTruthy();
      expect(panelBox!.x).toBeGreaterThanOrEqual(0);
      expect(panelBox!.x + panelBox!.width).toBeLessThanOrEqual(390);
      expect(panelBox!.y, "the notification panel must not cover its toggle").toBeGreaterThanOrEqual(
        bellBox!.y + bellBox!.height,
      );
      await readBell.click();
      await expect(panel).toHaveCount(0);
    }
    expect(seen.csp).toEqual([]);
    expect(seen.icons).toEqual([]);
  } finally {
    await m.context.close();
  }
});

test("logout ends the session and lands on the login page from both Vue pages; a failed logout keeps it", async ({
  page,
}) => {
  await ensureSetup(page);
  const seen = watchPage(page);
  const wsId = await workspaceId(page.request);
  await ganttProject(page.request, wsId);
  const doc = await createDoc(page.request, wsId, "셸 로그아웃");

  // A transport failure shows in place and keeps the session.
  await openWiki(page, doc);
  await page.route("**/api/v1/auth/logout", (route) => route.abort("connectionfailed"));
  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "연결을 확인하고 다시 시도해 주세요." })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`${doc.path}$`));
  expect((await page.request.get("/api/v1/auth/me")).ok()).toBe(true);
  await page.unroute("**/api/v1/auth/logout");

  for (const open of [openGantt, (p: Page) => openWiki(p, doc)]) {
    await open(page);
    const returnTo = new URL(page.url());
    const logoutRequest = page.waitForRequest(
      (request: Request) => request.url().endsWith("/api/v1/auth/logout") && request.method() === "POST",
    );
    await page.getByRole("button", { name: "로그아웃" }).click();
    // No push subscription in this browser: nothing to report.
    expect((await logoutRequest).postDataJSON()).toEqual({});
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole("button", { name: "로그인", exact: true })).toBeVisible();
    expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
    // The Vue page itself now sends the signed-out browser through login.
    await page.goto(`${returnTo.pathname}${returnTo.search}`);
    await expect(page).toHaveURL(/\/login\?returnTo=/);
    expect(new URL(page.url()).searchParams.get("returnTo")).toBe(`${returnTo.pathname}${returnTo.search}`);
    await login(page, admin.email, admin.password);
  }
  expect(seen.csp).toEqual([]);
  expect(seen.icons).toEqual([]);
});

test("the Vue shell re-binds this browser's push subscription to a new session, and logout disconnects it", async ({
  browser,
  baseURL,
}) => {
  test.setTimeout(120_000);
  const context = await browser.newContext({ baseURL });
  await context.grantPermissions(["notifications"]);
  // Headless Chromium has no push service: PushManager.subscribe and
  // getSubscription are a controlled in-page boundary kept in sessionStorage
  // (as in notifications-flow.spec.ts); the service worker, the /instance key
  // and every API route are real.
  await context.addInitScript(() => {
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
      const stored = { endpoint: `https://push.e2e.invalid/send/${crypto.randomUUID()}`, key: toBase64Url(key) };
      log("subscribe");
      sessionStorage.setItem(KEY, JSON.stringify(stored));
      return build(stored) as unknown as PushSubscription;
    };
  });
  const page = await context.newPage();
  const seen = watchPage(page);
  try {
    await login(page, admin.email, admin.password);
    const wsId = await workspaceId(page.request);
    await ganttProject(page.request, wsId);
    const doc = await createDoc(page.request, wsId, "셸 푸시");
    const putPath = `/api/v1/workspaces/${wsId}/push-subscriptions`;
    const isPut = (response: Response) => response.url().endsWith(putPath) && response.request().method() === "PUT";
    const pushLog = async (): Promise<string[]> =>
      JSON.parse((await page.evaluate(() => sessionStorage.getItem("e2e-push-log"))) ?? "[]");

    // Push is switched on in the React settings (session one).
    await page.goto(`/w/${admin.workspaceSlug}/settings`);
    const toggle = page.getByLabel("브라우저 푸시");
    await expect(toggle).toBeEnabled({ timeout: 15_000 });
    const saved = page.waitForResponse(isPut);
    await toggle.click();
    const endpoint: string = (await saved).request().postDataJSON().endpoint;
    await expect(toggle).toBeChecked();

    // Each new session re-binds the subscription when a Vue page opens, once.
    for (const open of [openGantt, (p: Page) => openWiki(p, doc)]) {
      await context.clearCookies();
      await login(page, admin.email, admin.password);
      const rebound = page.waitForResponse(isPut);
      await open(page);
      const put = await rebound;
      expect(put.status()).toBe(200);
      expect(put.request().postDataJSON().endpoint).toBe(endpoint);
    }
    expect(await pushLog()).toEqual(["subscribe"]);

    // Logout reports this browser's endpoint and drops the subscription.
    const logoutRequest = page.waitForRequest(
      (request: Request) => request.url().endsWith("/api/v1/auth/logout") && request.method() === "POST",
    );
    await page.getByRole("button", { name: "로그아웃" }).click();
    expect((await logoutRequest).postDataJSON()).toEqual({ pushEndpoint: endpoint });
    await expect(page).toHaveURL(/\/login$/);
    expect(await pushLog()).toEqual(["subscribe", "unsubscribe"]);
    expect(seen.csp).toEqual([]);
    expect(seen.icons).toEqual([]);
  } finally {
    await context.close();
  }
});

test("the workspace switch lands on the same section of the other workspace from both Vue pages", async ({
  page,
}) => {
  await ensureSetup(page);
  const seen = watchPage(page);
  const wsId = await workspaceId(page.request);
  await ganttProject(page.request, wsId);
  const doc = await createDoc(page.request, wsId, "셸 전환");

  // With one workspace the header names it.
  await openWiki(page, doc);
  await expect(page.locator('[data-slot="workspace-name"]')).toHaveText(admin.workspaceName);
  await expect(page.getByLabel("워크스페이스 전환")).toHaveCount(0);

  const created = await page.request.post("/api/v1/workspaces", {
    data: { name: "Vue Shell Two", slug: "vshell2" },
  });
  expect(created.status(), await created.text()).toBe(201);

  for (const [open, section] of [
    [openGantt, "projects"],
    [(p: Page) => openWiki(p, doc), "wiki"],
  ] as const) {
    await open(page);
    const select = page.getByLabel("워크스페이스 전환");
    await expect(select).toHaveValue(wsId);
    await markDocument(page);
    await select.selectOption({ label: "Vue Shell Two" });
    await expect(page).toHaveURL(new RegExp(`/w/vshell2/${section}$`));
    await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
    expect(await sameDocument(page), "workspace switch uses the Vue router").toBe(true);
  }
  expect(seen.csp).toEqual([]);
  expect(seen.icons).toEqual([]);
});

test("long Korean workspace controls stay in the narrow viewport and search placeholder has light/dark contrast", async ({ page }) => {
  await ensureSetup(page);
  const wsId = await workspaceId(page.request);
  const original = await (await page.request.get(`/api/v1/workspaces/${wsId}`)).json();
  const renamed = await page.request.patch(`/api/v1/workspaces/${wsId}`, {
    data: { name: "한국어 팀 업무 계획과 문서 검토 워크스페이스" },
  });
  expect(renamed.ok()).toBe(true);
  try {
    const doc = await createDoc(page.request, wsId, "좁은 화면 셸 검토");
    await page.setViewportSize({ width: 390, height: 844 });
    await openWiki(page, doc);
    const header = page.locator("header").first();
    for (const control of [header.getByRole("button", { name: "검색", exact: true }), header.getByRole("button", { name: "알림", exact: true }), header.getByRole("link", { name: "계정", exact: true }), header.getByRole("button", { name: "로그아웃", exact: true })]) {
      const bounds = await control.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(390);
      await control.focus();
      expect(await page.locator(".document-page").evaluate((e) => e.getBoundingClientRect().left)).toBeGreaterThanOrEqual(0);
    }
    const search = header.getByRole("button", { name: "검색", exact: true });
    await search.focus(); await search.press("Enter");
    const dialog = page.getByRole("dialog", { name: "빠른 검색" });
    await expect(dialog).toBeVisible();
    for (const dark of [false, true]) {
      await page.evaluate((enabled) => document.documentElement.classList.toggle("dark", enabled), dark);
      const ratio = await dialog.evaluate((root) => {
        const canvas = document.createElement("canvas");
        const context = canvas.getContext("2d")!;
        const luminance = (css: string) => {
          context.fillStyle = css; context.fillRect(0, 0, 1, 1);
          const rgb = [...context.getImageData(0, 0, 1, 1).data].slice(0, 3).map((v) => {
            const n = v / 255; return n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4;
          });
          return 0.2126 * rgb[0]! + 0.7152 * rgb[1]! + 0.0722 * rgb[2]!;
        };
        const fg = luminance(getComputedStyle(root.querySelector("input")!, "::placeholder").color);
        const bg = luminance(getComputedStyle(root).backgroundColor);
        return (Math.max(fg, bg) + 0.05) / (Math.min(fg, bg) + 0.05);
      });
      expect(ratio, dark ? "dark placeholder" : "light placeholder").toBeGreaterThanOrEqual(4.5);
    }
    await page.evaluate(() => document.documentElement.classList.remove("dark"));
    await page.keyboard.press("Escape"); await expect(search).toBeFocused();
  } finally {
    expect((await page.request.patch(`/api/v1/workspaces/${wsId}`, { data: { name: original.name } })).ok()).toBe(true);
  }
});
