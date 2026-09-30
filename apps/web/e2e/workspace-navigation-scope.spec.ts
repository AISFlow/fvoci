import { execFileSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login } from "./helpers";

test.describe.configure({ mode: "serial" });
const owner = { email: "scope@example.com", password: "scopepass123" };
let a: { id: string; slug: string };
let b: { id: string; slug: string };
let source: { id: string };
let aWiki: { id: string };
let bWiki: { id: string };

// Send the real request and hold only its genuine response while the user switches.
async function holdResponse(page: Page, path: string, method: string, succeeds = true) {
  let received!: () => void;
  let release!: () => void;
  const ready = new Promise<void>((resolve) => { received = resolve; });
  const released = new Promise<void>((resolve) => { release = resolve; });
  const glob = `**${path}`;
  const handler = async (route: import("@playwright/test").Route) => {
    if (route.request().method() !== method) return route.continue();
    const response = await route.fetch();
    expect(response.ok()).toBe(succeeds);
    received();
    await released;
    await route.fulfill({ response });
  };
  await page.route(glob, handler);
  return {
    ready,
    async finish() {
      const response = page.waitForResponse((response) => new URL(response.url()).pathname === path && response.request().method() === method);
      release();
      await (await response).finished();
      // Browser microtasks and one paint finish the real mutation callback.
      await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
      await page.unroute(glob, handler);
    },
    release,
  };
}

async function switchToB(page: Page, section: string): Promise<void> {
  // Browser Back can change workspace while a native modal makes the header inert.
  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`/w/${b.slug}/${section}$`));
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await expect(page.locator("main h1")).toBeVisible();
  await expect(page.locator("#workspace-switch")).toHaveValue(b.id);
}

async function prepareA(page: Page, section: string): Promise<void> {
  const resource = section === "wiki" ? "wiki-discovery" : section;
  const loaded = page.waitForResponse((response) => new URL(response.url()).pathname === `/api/v1/workspaces/${b.id}/${resource}`);
  await page.goto(`/w/${b.slug}/${section}`);
  await (await loaded).finished();
  await page.locator("#workspace-switch").selectOption(a.id);
  await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/${section}$`));
  await expect(page.locator("#workspace-switch")).toHaveValue(a.id);
}

function countBRequests(page: Page, resource: string): () => number {
  let count = 0;
  page.on("request", (request) => {
    if (request.method() === "GET" && new URL(request.url()).pathname === `/api/v1/workspaces/${b.id}/${resource}`) count++;
  });
  return () => count;
}

async function createProject(page: Page, workspaceId: string, key: string, name: string) {
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, { data: { key, name, visibility: "workspace" } });
  expect(response.status()).toBe(201);
  return response.json();
}

async function createWiki(page: Page, workspaceId: string, title: string) {
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, { data: { parentId: null, title } });
  expect(response.status()).toBe(201);
  return response.json();
}

test("two real workspaces share reference spellings without sharing resources", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("범위");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Scope A");
  await page.getByLabel("주소(영문)").fill("scope-a");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  a = (await (await page.request.get("/api/v1/me/workspaces")).json()).items[0];
  const created = await page.request.post("/api/v1/workspaces", { data: { name: "Scope B", slug: "scope-b" } });
  expect(created.status()).toBe(201);
  b = await created.json();
  source = await createProject(page, a.id, "SAME", "A source");
  await createProject(page, b.id, "INFLIGHT", "B existing");
  await createProject(page, b.id, "COPIED", "B copy reference");
  aWiki = await createWiki(page, a.id, "A wiki");
  bWiki = await createWiki(page, b.id, "B wiki");
  await createWiki(page, b.id, "B second wiki");
});

test("late project create and clone responses stay scoped to their original workspace", async ({ page }) => {
  await login(page, owner.email, owner.password);
  const count = countBRequests(page, "projects");
  for (const kind of ["create", "clone"] as const) {
    await prepareA(page, "projects");
    const path = kind === "create" ? `/api/v1/workspaces/${a.id}/projects` : `/api/v1/workspaces/${a.id}/projects/${source.id}/clone`;
    const gate = await holdResponse(page, path, "POST");
    try {
      if (kind === "create") await page.getByRole("button", { name: "새 프로젝트", exact: true }).click();
      else await page.locator(".project-list__row").filter({ has: page.locator(".project-list__key", { hasText: /^SAME$/ }) }).getByRole("button", { name: "복제", exact: true }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByLabel("키", { exact: true }).fill(kind === "create" ? "INFLIGHT" : "COPIED");
      if (kind === "create") await dialog.getByLabel("이름", { exact: true }).fill("A created");
      await dialog.getByRole("button", { name: kind === "create" ? "새 프로젝트" : "복제", exact: true }).click();
      await gate.ready;
      await switchToB(page, "projects");
      const baseline = count();
      await gate.finish();
      await expect(page).toHaveURL(new RegExp(`/w/${b.slug}/projects$`));
      expect(count()).toBe(baseline);
      await expect(page.getByRole("dialog")).toHaveCount(0);
      const projects = (await (await page.request.get(`/api/v1/workspaces/${a.id}/projects`)).json()).items;
      expect(projects.some((project: { key: string }) => project.key === (kind === "create" ? "INFLIGHT" : "COPIED"))).toBe(true);
    } finally { gate.release(); }
  }
});

test("project creation and cloning remain retired after returning A to B to A", async ({ page }) => {
  await login(page, owner.email, owner.password);
  for (const kind of ["create", "clone"] as const) {
    await prepareA(page, "projects");
    const path = kind === "create" ? `/api/v1/workspaces/${a.id}/projects` : `/api/v1/workspaces/${a.id}/projects/${source.id}/clone`;
    const gate = await holdResponse(page, path, "POST");
    try {
      if (kind === "create") await page.getByRole("button", { name: "새 프로젝트", exact: true }).click();
      else await page.locator(".project-list__row").filter({ has: page.locator(".project-list__key", { hasText: /^SAME$/ }) }).getByRole("button", { name: "복제", exact: true }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByLabel("키", { exact: true }).fill(kind === "create" ? "ABAPROJ" : "ABACOPY");
      if (kind === "create") await dialog.getByLabel("이름", { exact: true }).fill("Retired creation");
      await dialog.getByRole("button", { name: kind === "create" ? "새 프로젝트" : "복제", exact: true }).click();
      await gate.ready;
      await switchToB(page, "projects");
      await page.locator("#workspace-switch").selectOption(a.id);
      await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/projects$`));
      await gate.finish();
      await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/projects$`));
      await expect(page.getByRole("button", { name: "새 프로젝트", exact: true })).toBeEnabled();
    } finally { gate.release(); }
  }
});

test("a genuine late project conflict cannot show an error in the next workspace", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await prepareA(page, "projects");
  const gate = await holdResponse(page, `/api/v1/workspaces/${a.id}/projects`, "POST", false);
  try {
    await page.getByRole("button", { name: "새 프로젝트", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("키", { exact: true }).fill("SAME");
    await dialog.getByLabel("이름", { exact: true }).fill("Genuine duplicate");
    await dialog.getByRole("button", { name: "새 프로젝트", exact: true }).click();
    await gate.ready;
    await switchToB(page, "projects");
    await gate.finish();
    await expect(page).toHaveURL(new RegExp(`/w/${b.slug}/projects$`));
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByRole("main").getByRole("alert")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "새 프로젝트", exact: true })).toBeEnabled();
  } finally { gate.release(); }
});

test("late wiki creation cannot navigate to another workspace's matching document ref", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await prepareA(page, "wiki");
  const count = countBRequests(page, "wiki-discovery");
  const gate = await holdResponse(page, `/api/v1/workspaces/${a.id}/documents`, "POST");
  try {
    await page.getByRole("button", { name: "새 문서", exact: true }).click();
    await gate.ready;
    await switchToB(page, "wiki");
    const baseline = count();
    await gate.finish();
    await expect(page).toHaveURL(new RegExp(`/w/${b.slug}/wiki$`));
    expect(count()).toBe(baseline);
    await expect(page.getByText("B second wiki", { exact: true })).toBeVisible();
  } finally { gate.release(); }
});

test("wiki creation remains retired after A to B to A", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await prepareA(page, "wiki");
  const gate = await holdResponse(page, `/api/v1/workspaces/${a.id}/documents`, "POST");
  try {
    await page.getByRole("button", { name: "새 문서", exact: true }).click();
    await gate.ready;
    await switchToB(page, "wiki");
    await page.locator("#workspace-switch").selectOption(a.id);
    await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/wiki$`));
    await gate.finish();
    await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/wiki$`));
    await expect(page.getByRole("button", { name: "새 문서", exact: true })).toBeEnabled();
  } finally { gate.release(); }
});

test("late trash restoration refreshes A while B remains independently trashed", async ({ page }) => {
  await login(page, owner.email, owner.password);
  const docs = [await createWiki(page, a.id, "Scoped restore"), await createWiki(page, b.id, "Scoped restore")];
  for (let i = 0; i < 2; i++) expect((await page.request.post(`/api/v1/workspaces/${[a, b][i].id}/documents/${docs[i].id}/trash`)).ok()).toBe(true);
  await prepareA(page, "trash");
  const count = countBRequests(page, "trash");
  const gate = await holdResponse(page, `/api/v1/workspaces/${a.id}/documents/${docs[0].id}/restore`, "POST");
  try {
    await page.getByRole("button", { name: "복원 Scoped restore", exact: true }).click();
    await gate.ready;
    await switchToB(page, "trash");
    const baseline = count();
    await gate.finish();
    expect(count()).toBe(baseline);
    await expect(page.getByRole("button", { name: "복원 Scoped restore", exact: true })).toBeVisible();
    expect((await page.request.get(`/api/v1/workspaces/${a.id}/documents/${docs[0].id}`)).status()).toBe(200);
  } finally { gate.release(); }
});

test("late notification open and read-all preserve the other workspace's unread inbox", async ({ page, browser, baseURL }) => {
  await login(page, owner.email, owner.password);
  const userId = (await (await page.request.get("/api/v1/auth/me")).json()).userId;
  const commentIds: string[] = [];
  for (const [index, workspace] of [a, b].entries()) {
    const email = `scope-actor-${index}@example.com`;
    createE2eUser(email, "scopeactor123", "댓글", { workspaceSlug: workspace.slug, membershipRole: "member" });
    const context = await browser.newContext({ baseURL });
    try {
      const actor = await context.newPage();
      await login(actor, email, "scopeactor123");
      const response = await actor.request.post(`/api/v1/workspaces/${workspace.id}/documents/${[aWiki, bWiki][index].id}/comments`, { data: { body: "Scoped notification", mentionedUserIds: [userId] } });
      expect(response.status()).toBe(201);
      commentIds.push((await response.json()).id);
    } finally { await context.close(); }
  }
  // Same approved real-event fixture as workspace-navigation-vue: no outbox timing shortcut in the request paths under test.
  for (const id of [userId, ...commentIds]) expect(id).toMatch(/^[0-9a-f-]{36}$/i);
  execFileSync("docker", ["exec", "-i", process.env.FVOCI_TEST_PG_CONTAINER!, "psql", "-U", "postgres", "-d", new URL(process.env.FVOCI_E2E_ADMIN_DATABASE_URL!).pathname.slice(1), "-v", "ON_ERROR_STOP=1"], { input: `INSERT INTO fvoci.notifications (workspace_id,user_id,event_id,verb,actor_user_id,target_type,target_id,payload) SELECT e.workspace_id,'${userId}'::uuid,e.id,e.verb,e.actor_user_id,e.target_type,e.target_id,jsonb_build_object('commentId',c.id,'documentId',c.document_id,'parentId',NULL) FROM fvoci.events e JOIN fvoci.comments c ON c.id=e.target_id WHERE e.target_id IN (${commentIds.map((id) => `'${id}'::uuid`).join(",")}) AND e.verb='comment.created' ON CONFLICT (workspace_id,user_id,event_id) DO NOTHING;`, stdio: ["pipe", "pipe", "pipe"] });
  for (const operation of ["open", "read-all"] as const) {
    const notification = (await (await page.request.get(`/api/v1/workspaces/${a.id}/notifications`)).json()).items[0];
    expect((await page.request.patch(`/api/v1/workspaces/${a.id}/notifications/${notification.id}`, { data: { read: false } })).ok()).toBe(true);
    await prepareA(page, "notifications");
    await expect(page.locator(".notifications-page__row")).toHaveCount(1);
    const count = countBRequests(page, "notifications");
    const path = operation === "open" ? `/api/v1/workspaces/${a.id}/notifications/${notification.id}` : `/api/v1/workspaces/${a.id}/notifications/read-all`;
    const gate = await holdResponse(page, path, operation === "open" ? "PATCH" : "POST");
    try {
      await (operation === "open" ? page.locator(".notifications-page__item") : page.getByRole("button", { name: "전체 읽음", exact: true })).click();
      await gate.ready;
      await switchToB(page, "notifications");
      const baseline = count();
      await gate.finish();
      await expect(page).toHaveURL(new RegExp(`/w/${b.slug}/notifications$`));
      expect(count()).toBe(baseline);
      expect((await (await page.request.get(`/api/v1/workspaces/${b.id}/notifications/unread-count`)).json()).count).toBe(1);
      await expect(page.getByRole("button", { name: "전체 읽음", exact: true })).toBeEnabled();
    } finally { gate.release(); }
  }
});

test("notification actions remain retired after A to B to A", async ({ page }) => {
  await login(page, owner.email, owner.password);
  for (const operation of ["open", "read-all"] as const) {
    const notification = (await (await page.request.get(`/api/v1/workspaces/${a.id}/notifications`)).json()).items[0];
    expect((await page.request.patch(`/api/v1/workspaces/${a.id}/notifications/${notification.id}`, { data: { read: false } })).ok()).toBe(true);
    await prepareA(page, "notifications");
    await expect(page.locator(".notifications-page__row")).toHaveCount(1);
    const path = operation === "open" ? `/api/v1/workspaces/${a.id}/notifications/${notification.id}` : `/api/v1/workspaces/${a.id}/notifications/read-all`;
    const gate = await holdResponse(page, path, operation === "open" ? "PATCH" : "POST");
    try {
      await (operation === "open" ? page.locator(".notifications-page__item") : page.getByRole("button", { name: "전체 읽음", exact: true })).click();
      await gate.ready;
      await switchToB(page, "notifications");
      await page.locator("#workspace-switch").selectOption(a.id);
      await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/notifications$`));
      await gate.finish();
      await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/notifications$`));
      await expect(page.getByRole("button", { name: "전체 읽음", exact: true })).toBeEnabled();
    } finally { gate.release(); }
  }
});

test("departing the section retires a pending create even with the same workspace", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await page.goto(`/w/${a.slug}/wiki`);
  await page.getByRole("navigation", { name: "워크스페이스" }).getByRole("link", { name: "프로젝트", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/projects$`));
  const gate = await holdResponse(page, `/api/v1/workspaces/${a.id}/projects`, "POST");
  try {
    await page.getByRole("button", { name: "새 프로젝트", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("키", { exact: true }).fill("DETACHED");
    await dialog.getByLabel("이름", { exact: true }).fill("Retired section");
    await dialog.getByRole("button", { name: "새 프로젝트", exact: true }).click();
    await gate.ready;
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/wiki$`));
    await gate.finish();
    await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/wiki$`));
    await expect(page.getByRole("heading", { name: "위키", exact: true })).toBeVisible();
  } finally { gate.release(); }
});

test("a real workspace role change retires an owner operation without late navigation", async ({ page, browser, baseURL }) => {
  await login(page, owner.email, owner.password);
  const userId = (await (await page.request.get("/api/v1/auth/me")).json()).userId;
  createE2eUser("scope-owner@example.com", "scopeowner123", "관리", { workspaceSlug: a.slug, membershipRole: "owner" });
  const context = await browser.newContext({ baseURL });
  const actor = await context.newPage();
  await login(actor, "scope-owner@example.com", "scopeowner123");
  await page.goto(`/w/${a.slug}/projects`);
  const gate = await holdResponse(page, `/api/v1/workspaces/${a.id}/projects`, "POST");
  try {
    await page.getByRole("button", { name: "새 프로젝트", exact: true }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("키", { exact: true }).fill("OLDROLE");
    await dialog.getByLabel("이름", { exact: true }).fill("Retired authority");
    await dialog.getByRole("button", { name: "새 프로젝트", exact: true }).click();
    await gate.ready;
    const refreshed = page.waitForResponse(response => new URL(response.url()).pathname === "/api/v1/me/workspaces" && response.ok());
    const changed = await actor.request.patch(`/api/v1/workspaces/${a.id}/members/${userId}`, { data: { role: "guest" } });
    expect(changed.ok(), await changed.text()).toBe(true);
    const list = await (await refreshed).json();
    expect(list.items.find((workspace: { id: string }) => workspace.id === a.id).role).toBe("guest");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await gate.finish();
    await expect(page).toHaveURL(new RegExp(`/w/${a.slug}/projects$`));
    await expect(page.getByRole("main").getByRole("alert")).toHaveCount(0);
  } finally {
    gate.release();
    const restored = await actor.request.patch(`/api/v1/workspaces/${a.id}/members/${userId}`, { data: { role: "owner" } });
    expect(restored.ok(), await restored.text()).toBe(true);
    await context.close();
  }
});
