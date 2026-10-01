import { expect, test, type APIRequestContext, type Page, type WebSocket } from "@playwright/test";
import type { QueryClient } from "@tanstack/vue-query";
import type { Router } from "vue-router";
import { z } from "zod";
import { createE2eUser, flowSchemas, login, readJson } from "./helpers";

// Production Vue/Rust + isolated PostgreSQL group. The private Vue property is
// read only by this test to prove the mounted QueryClient survives navigation.
type AppRoot = HTMLElement & {
  __vue_app__: {
    _context: { provides: { VUE_QUERY_CLIENT: QueryClient } };
    config: { globalProperties: { $router: Router } };
  };
};
type ProbeDocument = Document & { task4Client?: QueryClient };
const slug = "navlife";
const admin = { email: "Admin@Example.COM", password: "supersecret1" };

async function setup(page: Page): Promise<string> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃", exact: true }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("이동");
    await page.getByLabel("이메일").fill(admin.email);
    await page.getByLabel("비밀번호").fill(admin.password);
    await page.getByLabel("워크스페이스 이름").fill("Navigation lifetime");
    await page.getByLabel("주소(영문)").fill(slug);
    await page.getByRole("button", { name: "시작하기" }).click();
  } else if (await page.getByRole("button", { name: "로그인", exact: true }).count()) {
    await login(page, admin.email, admin.password);
  }
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
  const response = await page.request.get("/api/v1/me/workspaces");
  expect(response.ok()).toBe(true);
  const workspace = (await readJson(response, flowSchemas.workspaces)).items.find(
    (item) => item.slug === slug,
  );
  if (!workspace) throw new Error("Missing isolated workspace");
  return workspace.id;
}

async function markLifetime(page: Page): Promise<string> {
  return page.evaluate(() => {
    const root = document.getElementById("root") as AppRoot | null;
    if (!root?.__vue_app__) throw new Error("Vue app is not mounted");
    const client = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
    const token = crypto.randomUUID();
    document.documentElement.dataset.task4Document = token;
    (document as ProbeDocument).task4Client = client;
    client.setQueryData(["task4-lifetime-proof"], token);
    return token;
  });
}

async function expectLifetime(page: Page, token: string): Promise<void> {
  await expect(page.locator("html")).toHaveAttribute("data-task4-document", token);
  const state = await page.evaluate(() => {
    const root = document.getElementById("root") as AppRoot;
    const client = root.__vue_app__._context.provides.VUE_QUERY_CLIENT;
    return {
      sameClient: client === (document as ProbeDocument).task4Client,
      cached: client.getQueryData(["task4-lifetime-proof"]),
    };
  });
  expect(state).toEqual({ sameClient: true, cached: token });
}

async function push(page: Page, path: string): Promise<void> {
  await page.evaluate(async (destination) => {
    const root = document.getElementById("root") as AppRoot;
    await root.__vue_app__.config.globalProperties.$router.push(destination);
  }, path);
}

function sockets(page: Page): Set<WebSocket> {
  const open = new Set<WebSocket>();
  page.on("websocket", (socket) => {
    if (new URL(socket.url()).pathname !== "/collab") return;
    open.add(socket);
    socket.on("close", () => open.delete(socket));
  });
  return open;
}

async function createWiki(request: APIRequestContext, workspaceId: string, title: string) {
  const response = await request.post(`/api/v1/workspaces/${workspaceId}/documents`, {
    data: { parentId: null, title },
  });
  expect(response.status()).toBe(201);
  return readJson(response, flowSchemas.document);
}

test("missing wiki replaces history while retaining the browser document and mounted cache", async ({
  page,
}) => {
  await setup(page);
  await page.goto(`/w/${slug}/wiki`);
  await expect(page.getByRole("heading", { name: "위키", exact: true })).toBeVisible();
  const token = await markLifetime(page);
  const history = await page.evaluate(() => window.history.length);
  await push(page, `/w/${slug}/WIKI-999999`);
  await expect(page).toHaveURL(`/w/${slug}/wiki`);
  await expectLifetime(page, token);
  expect(await page.evaluate(() => window.history.length)).toBe(history + 1);
  await page.goBack();
  await expect(page).toHaveURL(`/w/${slug}/wiki`);
  await expectLifetime(page, token);
});

for (const kind of ["wiki", "project"] as const) {
  test(`${kind} trash pushes history, closes the room and preserves a saved last edit through restore/back/fresh read`, async ({
    page,
    browser,
    baseURL,
  }) => {
    const workspaceId = await setup(page);
    const title = `Lifetime ${kind}`;
    let documentId: string;
    let documentPath: string;
    let bodyPath: string;
    let trashEndpoint: string;
    if (kind === "wiki") {
      const doc = await createWiki(page.request, workspaceId, title);
      documentId = doc.id;
      documentPath = `/w/${slug}/WIKI-${String(doc.number)}`;
      bodyPath = `/api/v1/workspaces/${workspaceId}/documents/${documentId}/body`;
      trashEndpoint = `/api/v1/workspaces/${workspaceId}/documents/${documentId}/trash`;
    } else {
      const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, {
        data: { key: "NAVL", name: "Navigation project", visibility: "workspace" },
      });
      expect(response.status()).toBe(201);
      const project = await readJson(response, flowSchemas.project);
      const created = await page.request.post(
        `/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents`,
        {
          data: { parentId: project.rootDocumentId, title },
        },
      );
      expect(created.status()).toBe(201);
      const doc = await readJson(created, flowSchemas.createdDocument);
      documentId = doc.id;
      documentPath = `/w/${slug}/${doc.displayId}`;
      bodyPath = `/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents/${documentId}/body`;
      trashEndpoint = `/api/v1/workspaces/${workspaceId}/projects/${project.id}/documents/${documentId}/trash`;
    }
    const open = sockets(page);
    const primedTrash = page.waitForResponse(
      (response) =>
        response.request().method() === "GET" &&
        new URL(response.url()).pathname === `/api/v1/workspaces/${workspaceId}/trash` &&
        response.ok(),
    );
    await page.goto(`/w/${slug}/trash`);
    await primedTrash;
    await expect(page.getByText("휴지통이 비었습니다")).toBeVisible();
    // The destination's fresh empty cache must be invalidated by trash even
    // when the app survives and that query is still inside its staleTime.
    await push(page, documentPath);
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
    const editor = page.locator(".fvoci-editor .ProseMirror");
    await expect(editor).toBeVisible();
    const text = `durable last edit ${kind}`;
    await editor.click();
    await page.keyboard.type(text);
    await page.getByRole("button", { name: "저장", exact: true }).click();
    await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15000 });
    expect(open.size).toBe(1);
    const token = await markLifetime(page);
    const history = await page.evaluate(() => window.history.length);
    await page.evaluate(() => {
      window.confirm = () => true;
    });
    await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
    const trashed = page.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === trashEndpoint &&
        response.ok(),
    );
    await page.getByRole("button", { name: "휴지통으로 이동" }).click();
    await trashed;
    await expect(page).toHaveURL(`/w/${slug}/trash`);
    await expectLifetime(page, token);
    expect(await page.evaluate(() => window.history.length)).toBe(history + 1);
    await expect.poll(() => open.size).toBe(0);
    await expect(page.getByRole("button", { name: `복원 ${title}`, exact: true })).toBeVisible();
    await page.getByRole("button", { name: `복원 ${title}`, exact: true }).click();
    await expect(page.getByRole("button", { name: `복원 ${title}`, exact: true })).toHaveCount(0);
    await page.goBack();
    await expect(page).toHaveURL(documentPath);
    await expectLifetime(page, token);
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
    await expect(editor).toContainText(text);
    await expect.poll(() => open.size).toBe(1);
    const body = await page.request.get(bodyPath);
    expect(body.ok()).toBe(true);
    expect(JSON.stringify((await readJson(body, flowSchemas.body)).contentJson)).toContain(text);
    const fresh = await browser.newContext({ baseURL });
    try {
      const reader = await fresh.newPage();
      await login(reader, admin.email, admin.password);
      await reader.goto(documentPath);
      await expect(reader.locator(".fvoci-editor .ProseMirror")).toContainText(text);
      await expect(reader.locator("html")).not.toHaveAttribute("data-task4-document");
    } finally {
      await fresh.close();
    }
  });
}

test("wiki tree 5xx offers retry without denial or navigation, then resolves a missing ref in the same document", async ({
  page,
}) => {
  const workspaceId = await setup(page);
  const token = await markLifetime(page);
  let failing = true;
  await page.route(`**/api/v1/workspaces/${workspaceId}/tree`, async (route) => {
    if (failing)
      await route.fulfill({
        status: 503,
        contentType: "application/problem+json",
        body: JSON.stringify({ status: 503, title: "Temporary outage" }),
      });
    else await route.continue();
  });
  const path = `/w/${slug}/WIKI-999998`;
  await push(page, path);
  await expect(page.getByRole("alert")).toBeVisible({ timeout: 15000 });
  await expect(page).toHaveURL(path);
  await expectLifetime(page, token);
  failing = false;
  await page.getByRole("button", { name: "다시 시도", exact: true }).click();
  await expect(page).toHaveURL(`/w/${slug}/wiki`);
  await expectLifetime(page, token);
});

for (const outcome of ["success", "503"] as const) {
  test(`a late wiki trash ${outcome} cannot navigate or change the next document`, async ({
    page,
  }) => {
    const workspaceId = await setup(page);
    const old = await createWiki(page.request, workspaceId, `Old operation ${outcome}`);
    const current = await createWiki(page.request, workspaceId, `Current operation ${outcome}`);
    const oldPath = `/w/${slug}/WIKI-${String(old.number)}`;
    const currentPath = `/w/${slug}/WIKI-${String(current.number)}`;
    const endpoint = `/api/v1/workspaces/${workspaceId}/documents/${old.id}/trash`;
    await page.goto(oldPath);
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
    const token = await markLifetime(page);
    let release!: () => void;
    let observed!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const received = new Promise<void>((resolve) => {
      observed = resolve;
    });
    await page.route(`**${endpoint}`, async (route) => {
      const response = outcome === "success" ? await route.fetch() : undefined;
      if (response) expect(response.ok()).toBe(true);
      observed();
      await held;
      if (response) await route.fulfill({ response });
      else
        await route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ status: 503, title: "Late outage" }),
        });
    });
    try {
      await page.evaluate(() => {
        window.confirm = () => true;
      });
      await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
      await page.getByRole("button", { name: "휴지통으로 이동" }).click();
      await received;
      await push(page, currentPath);
      await expect(page.getByLabel("문서 제목")).toHaveValue(current.title);
      await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({
        timeout: 15000,
      });
      release();
      await expect
        .poll(() =>
          page.evaluate(() =>
            (
              document.getElementById("root") as AppRoot
            ).__vue_app__._context.provides.VUE_QUERY_CLIENT.isMutating(),
          ),
        )
        .toBe(0);
      await expect(page).toHaveURL(currentPath);
      await expect(page.getByLabel("문서 제목")).toHaveValue(current.title);
      await expect(page.getByRole("alert")).toHaveCount(0);
      await expectLifetime(page, token);
    } finally {
      release();
    }
  });
}

test("an authenticated actor change closes the old room and hard-reenters without exposing its mounted cache", async ({
  page,
}) => {
  const workspaceId = await setup(page);
  const other = { email: "navigation-actor@example.com", password: "memberpass1" };
  createE2eUser(other.email, other.password, "다른 사용자", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const doc = await createWiki(page.request, workspaceId, "Actor boundary");
  const path = `/w/${slug}/WIKI-${String(doc.number)}`;
  const open = sockets(page);
  await page.goto(path);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
  const token = await markLifetime(page);
  const loginResponse = await page.request.post("/api/v1/auth/login", {
    data: { email: admin.email, password: admin.password },
  });
  expect(loginResponse.ok()).toBe(true);
  // Same actor / changed session cookie is an ordinary successful refetch.
  await page.evaluate(async () => {
    const client = (document.getElementById("root") as AppRoot).__vue_app__._context.provides
      .VUE_QUERY_CLIENT;
    await client.refetchQueries({ queryKey: ["auth", "me"] });
  });
  await expectLifetime(page, token);
  expect(open.size).toBe(1);
  const changed = await page.request.post("/api/v1/auth/login", { data: other });
  expect(changed.ok()).toBe(true);
  const reentry = page.waitForEvent("request", {
    predicate: (request) => request.isNavigationRequest() && request.frame() === page.mainFrame(),
  });
  await page.evaluate(() => {
    const client = (document.getElementById("root") as AppRoot).__vue_app__._context.provides
      .VUE_QUERY_CLIENT;
    client.refetchQueries({ queryKey: ["auth", "me"] }).catch(reportError);
  });
  expect((await (await reentry).response())?.status()).toBe(200);
  await expect(page).toHaveURL(path);
  await expect(page.getByLabel("문서 제목")).toHaveValue("Actor boundary");
  await expect(page.locator("html")).not.toHaveAttribute("data-task4-document", token);
  expect(
    await page.evaluate(() =>
      (
        document.getElementById("root") as AppRoot
      ).__vue_app__._context.provides.VUE_QUERY_CLIENT.getQueryData(["task4-lifetime-proof"]),
    ),
  ).toBeUndefined();
  await expect.poll(() => open.size).toBe(1);
  // Logout remains a hard boundary, with its existing server and push cleanup.
  await page.getByRole("button", { name: "로그아웃", exact: true }).click();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByLabel("이메일")).toBeVisible();
  await expect(page.locator("html")).not.toHaveAttribute("data-task4-document", token);
  await expect.poll(() => open.size).toBe(0);
});

test("a missing-wiki lazy chunk failure stays visible on its owning page without reloading", async ({
  page,
}) => {
  await setup(page);
  const token = await markLifetime(page);
  let requests = 0;
  await page.route("**/assets/WikiPage-*.js", async (route) => {
    requests += 1;
    await route.abort("failed");
  });
  const path = `/w/${slug}/WIKI-999997`;
  await push(page, path);
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(path);
  await expectLifetime(page, token);
  expect(requests).toBe(1);
});

test("a late missing-wiki chunk rejection cannot replace the next document or display its error there", async ({
  page,
}) => {
  const workspaceId = await setup(page);
  const doc = await createWiki(page.request, workspaceId, "Current after missing");
  const token = await markLifetime(page);
  let release!: () => void;
  let observed!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const requested = new Promise<void>((resolve) => {
    observed = resolve;
  });
  const rejected = page.waitForEvent("requestfailed", {
    predicate: (request) => /\/assets\/WikiPage-[^/]+\.js$/.test(new URL(request.url()).pathname),
  });
  await page.route("**/assets/WikiPage-*.js", async (route) => {
    observed();
    await held;
    await route.abort("failed");
  });
  try {
    await push(page, `/w/${slug}/WIKI-999996`);
    await requested;
    const path = `/w/${slug}/WIKI-${String(doc.number)}`;
    await push(page, path);
    await expect(page.getByLabel("문서 제목")).toHaveValue(doc.title);
    release();
    await rejected;
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => {
          requestAnimationFrame(() => {
            resolve();
          });
        }),
    );
    await expect(page).toHaveURL(path);
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expectLifetime(page, token);
  } finally {
    release();
  }
});

test("actual workspace membership revocation closes the room and keeps the permission eviction hard boundary", async ({
  page,
  browser,
  baseURL,
}) => {
  const workspaceId = await setup(page);
  const member = { email: "navigation-revoked@example.com", password: "memberpass1" };
  createE2eUser(member.email, member.password, "철회 사용자", {
    workspaceSlug: slug,
    membershipRole: "member",
  });
  const doc = await createWiki(page.request, workspaceId, "Revoked workspace");
  const context = await browser.newContext({ baseURL });
  try {
    const reader = await context.newPage();
    await login(reader, member.email, member.password);
    const meResponse = await reader.request.get("/api/v1/auth/me");
    expect(meResponse.ok()).toBe(true);
    const me = await readJson(meResponse, z.object({ userId: z.string() }));
    const open = sockets(reader);
    await reader.goto(`/w/${slug}/WIKI-${String(doc.number)}`);
    await expect(reader.locator('[data-collab-status="connected"]')).toBeVisible({
      timeout: 15000,
    });
    const token = await markLifetime(reader);
    const removed = await page.request.delete(
      `/api/v1/workspaces/${workspaceId}/members/${me.userId}`,
    );
    expect(removed.ok()).toBe(true);
    await expect(reader).toHaveURL(/\/\?denied=workspace$/, { timeout: 15000 });
    await expect(reader.locator("html")).not.toHaveAttribute("data-task4-document", token);
    await expect(reader.locator(".fvoci-editor .ProseMirror")).toHaveCount(0);
    await expect.poll(() => open.size).toBe(0);
  } finally {
    await context.close();
  }
});
