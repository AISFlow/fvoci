import { execFileSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";
import { createE2eUser, login, logout, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });
const owner = { email: "navigation@example.com", password: "navigation123" };
let workspaceId: string;
let projectId: string;

async function vue(page: Page): Promise<void> {
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
}

test("workspace entrance, project creation validation, clone and archived navigation survive reload", async ({ page }) => {
  const csp = watchCspViolations(page);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("탐색");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Navigation workspace");
  await page.getByLabel("주소(영문)").fill("navigation");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  workspaceId = (await (await page.request.get("/api/v1/me/workspaces")).json()).items.find(
    (item: { slug: string }) => item.slug === "navigation",
  ).id;
  await page.goto("/w/navigation?from=direct#entrance");
  await vue(page);
  await expect(page.getByRole("navigation", { name: "워크스페이스" }).getByRole("link", { name: "홈", exact: true })).toHaveAttribute("aria-current", "page");
  await page.getByRole("navigation", { name: "워크스페이스" }).getByRole("link", { name: "프로젝트", exact: true }).click();
  await vue(page);
  await page.getByRole("button", { name: "새 프로젝트", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("키", { exact: true }).fill("WIKI");
  await dialog.getByLabel("이름", { exact: true }).fill("Navigation project");
  await dialog.getByRole("button", { name: "새 프로젝트", exact: true }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  await dialog.getByLabel("키", { exact: true }).fill("NAV");
  await dialog.getByRole("button", { name: "새 프로젝트", exact: true }).click();
  await expect(page).toHaveURL(/\/NAV\/tasks$/);
  await vue(page);
  const projects = (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`)).json()).items;
  projectId = projects.find((item: { key: string }) => item.key === "NAV").id;
  await page.goto("/w/navigation/projects");
  await page.getByRole("button", { name: "복제", exact: true }).click();
  await dialog.getByLabel("키", { exact: true }).fill("COPY");
  await dialog.getByRole("button", { name: "복제", exact: true }).click();
  await expect(page).toHaveURL(/\/COPY\/tasks$/);
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${projectId}/archive`)).ok()).toBe(true);
  await page.goto("/w/navigation/projects");
  await expect(page.getByRole("list", { name: "보관됨" }).getByRole("link")).toContainText("Navigation project");
  await page.reload();
  await vue(page);
  await page.getByRole("list", { name: "보관됨" }).getByRole("link").click();
  await expect(page).toHaveURL(/\/NAV\/tasks$/);
  await vue(page);
  expect(csp).toEqual([]);
});

test("wiki list creates documents and restores wiki and project trash through the live Rust endpoints", async ({ page }) => {
  await login(page, owner.email, owner.password);
  await page.goto("/w/navigation/wiki");
  await vue(page);
  await page.getByRole("button", { name: "새 문서", exact: true }).click();
  await expect(page).toHaveURL(/\/WIKI-\d+$/);
  await vue(page);
  const wiki = (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/tree`)).json()).items[0];
  expect((await page.request.patch(`/api/v1/workspaces/${workspaceId}/documents/${wiki.id}`, { data: { title: "Restorable wiki" } })).ok()).toBe(true);
  expect((await page.request.delete(`/api/v1/workspaces/${workspaceId}/documents/${wiki.id}`)).ok()).toBe(true);
  const copy = (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`)).json()).items.find((item: { key: string }) => item.key === "COPY");
  const created = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${copy.id}/documents`, { data: { parentId: copy.rootDocumentId, title: "Restorable project document" } });
  expect(created.status()).toBe(201);
  const document = await created.json();
  expect((await page.request.delete(`/api/v1/workspaces/${workspaceId}/projects/${copy.id}/documents/${document.id}`)).ok()).toBe(true);
  await page.goto("/w/navigation/trash");
  await vue(page);
  await page.getByRole("button", { name: "복원 Restorable wiki", exact: true }).click();
  await expect(page.getByText("Restorable wiki", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "복원 Restorable project document", exact: true }).click();
  await expect(page.getByText("Restorable project document", { exact: true })).toHaveCount(0);
  await page.reload();
  await vue(page);
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/documents/${wiki.id}`)).ok()).toBe(true);
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/projects/${copy.id}/documents/${document.id}`)).ok()).toBe(true);
  await page.getByRole("link", { name: "위키로", exact: true }).click();
  await expect(page.getByRole("link", { name: /Restorable wiki/ })).toBeVisible();
});

test("direct section URLs preserve query and hash across reload; foreign and signed-out workspaces are denied", async ({ page }) => {
  await login(page, owner.email, owner.password);
  for (const path of ["/w/navigation", "/w/navigation/projects", "/w/navigation/wiki", "/w/navigation/my-tasks", "/w/navigation/notifications", "/w/navigation/trash", "/w/navigation/search?q=missing&tab=task"]) {
    const target = `${path}${path.includes("?") ? "&" : "?"}from=direct#section`;
    await page.goto(target);
    await vue(page);
    await page.reload();
    await vue(page);
    expect(new URL(page.url()).pathname + new URL(page.url()).search + new URL(page.url()).hash).toBe(target);
    await expect(page.getByRole("main").getByRole("heading", { level: 1 })).toBeVisible();
  }
  await page.goto("/w/navigation/search?q=missing&tab=task");
  await page.getByRole("tab", { name: "문서", exact: true }).click();
  await expect(page).toHaveURL(/tab=document/);
  await page.reload();
  await expect(page.getByRole("tab", { name: "문서", exact: true })).toHaveAttribute("aria-selected", "true");
  await logout(page);
  await page.goto("/w/navigation/my-tasks?from=login#mine");
  await expect(page).toHaveURL(/\/login\?returnTo=/);
  expect(new URL(page.url()).searchParams.get("returnTo")).toBe("/w/navigation/my-tasks?from=login#mine");
  createE2eUser("navigation-outsider@example.com", "outsiderpass123", "외부");
  await login(page, "navigation-outsider@example.com", "outsiderpass123");
  await page.goto("/w/navigation/projects");
  await expect(page).toHaveURL(/\?denied=workspace$/);
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`)).status()).toBe(404);
});

test("notification pagination reaches a third page, bell cache stays valid, and archive/read persist", async ({ page, browser, baseURL }) => {
  await login(page, owner.email, owner.password);
  createE2eUser("navigation-inbox@example.com", "inboxpass123", "수신", { workspaceSlug: "navigation", membershipRole: "member" });
  const members = (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/members`)).json()).items;
  const memberId = members.find((item: { email: string }) => item.email === "navigation-inbox@example.com").userId;
  const copy = (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`)).json()).items.find((item: { key: string }) => item.key === "COPY");
  createE2eUser("navigation-mentions@example.com", "mentionspass123", "댓글", { workspaceSlug: "navigation", membershipRole: "member" });
  const authorContext = await browser.newContext({ baseURL });
  const coauthor = await authorContext.newPage();
  const commentIds: string[] = [];
  try {
    await login(coauthor, "navigation-mentions@example.com", "mentionspass123");
    for (let start = 0; start < 105; start += 5) {
      await Promise.all(Array.from({ length: Math.min(5, 105 - start) }, async (_, offset) => {
        // Two real members stay within the unchanged sixty-comments/user limit.
        const author = start + offset < 53 ? page : coauthor;
        const comment = await author.request.post(`/api/v1/workspaces/${workspaceId}/projects/${copy.id}/documents/${copy.rootDocumentId}/comments`, {
          data: { body: `Paged inbox ${start + offset}`, mentionedUserIds: [memberId] },
        });
        expect(comment.status()).toBe(201);
        commentIds.push((await comment.json()).id);
      }));
    }
  } finally {
    await authorContext.close();
  }
  // Coordinator-approved DB fixture materializes these genuine API-created
  // events in the isolated inbox. The unchanged notifications-flow separately
  // covers event delivery; this flow proves real HTTP cursors/read/archive.
  for (const id of [workspaceId, memberId, ...commentIds]) expect(id).toMatch(/^[0-9a-f-]{36}$/i);
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  if (!container || !adminUrl) throw new Error("isolated PostgreSQL fixture context is required");
  const sql = `
    BEGIN;
    INSERT INTO fvoci.notifications (workspace_id, user_id, event_id, verb, actor_user_id, target_type, target_id, payload, created_at)
    SELECT e.workspace_id, '${memberId}'::uuid, e.id, e.verb, e.actor_user_id, e.target_type, e.target_id,
      jsonb_strip_nulls(jsonb_build_object('commentId', c.id, 'documentId', c.document_id, 'taskId', c.task_id)) || jsonb_build_object('parentId', c.parent_id), e.created_at
    FROM fvoci.events e JOIN fvoci.comments c ON c.id = e.target_id AND c.workspace_id = e.workspace_id
    WHERE e.workspace_id = '${workspaceId}'::uuid AND e.verb = 'comment.created'
      AND e.target_id IN (${commentIds.map((id) => `'${id}'::uuid`).join(",")})
    ON CONFLICT (workspace_id, user_id, event_id) DO NOTHING;
    COMMIT;
  `;
  execFileSync("docker", ["exec", "-i", container, "psql", "-U", "postgres", "-d", new URL(adminUrl).pathname.slice(1), "-v", "ON_ERROR_STOP=1"], { input: sql, stdio: ["pipe", "pipe", "pipe"] });
  await logout(page);
  await login(page, "navigation-inbox@example.com", "inboxpass123");
  expect((await (await page.request.get(`/api/v1/workspaces/${workspaceId}/notifications/unread-count`)).json()).count).toBe(105);
  await page.goto("/w/navigation/notifications");
  await vue(page);
  const rows = page.locator(".notifications-page__row");
  await expect(rows).toHaveCount(50);
  await page.getByRole("button", { name: "더 보기", exact: true }).click();
  await expect(rows).toHaveCount(100);
  await page.getByRole("button", { name: "더 보기", exact: true }).click();
  await expect(rows).toHaveCount(105);
  await expect(page.getByRole("button", { name: "더 보기", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: /안 읽은 알림 105건/ }).click();
  await expect(page.getByRole("region", { name: "알림", exact: true }).getByText("문서에 새 댓글이 달렸습니다", { exact: true }).first()).toBeVisible();
  await page.getByRole("button", { name: /안 읽은 알림 105건/ }).click();
  await rows.first().getByRole("button", { name: "보관", exact: true }).click();
  await page.getByRole("tab", { name: "보관", exact: true }).click();
  await expect(rows).toHaveCount(1);
  await expect(page).toHaveURL(/\?tab=archived$/);
  await page.reload();
  await expect(page.getByRole("tab", { name: "보관", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(rows).toHaveCount(1);
  await rows.first().getByRole("button", { name: "보관 해제", exact: true }).click();
  await expect(rows).toHaveCount(0);
  await page.getByRole("button", { name: "전체 읽음", exact: true }).click();
  await page.getByRole("tab", { name: "안 읽음", exact: true }).click();
  await expect(page).toHaveURL(/\?tab=unread$/);
  await page.reload();
  await expect(page.getByRole("tab", { name: "안 읽음", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(rows).toHaveCount(0);
  expect((await (await page.request.get(`/api/v1/workspaces/${workspaceId}/notifications/unread-count`)).json()).count).toBe(0);
});
