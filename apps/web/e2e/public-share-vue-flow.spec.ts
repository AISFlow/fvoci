import { execFileSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";
import { watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });

function fixtureSql(sql: string): void {
  const admin = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!admin || !container) throw new Error("isolated Rust/PostgreSQL fixture required");
  execFileSync("docker", ["exec", "-i", container, "psql", "-U", "postgres", "-d",
    new URL(admin).pathname.slice(1), "-v", "ON_ERROR_STOP=1"], { input: sql, stdio: "pipe" });
}

function uuid(value: string): string {
  if (!/^[0-9a-f-]{36}$/i.test(value)) throw new Error("invalid fixture UUID");
  return `'${value}'`;
}

function seedContent(id: string, content: unknown, table = "documents"): void {
  const json = JSON.stringify(content).replaceAll("'", "''");
  fixtureSql(`UPDATE fvoci.${table} SET content_json = '${json}'::jsonb WHERE id = ${uuid(id)};`);
}

async function setup(page: Page): Promise<string> {
  const status = await page.request.get("/api/v1/setup");
  expect(status.ok()).toBe(true);
  if ((await status.json()).needed) {
    await page.goto("/");
    await expect(page).toHaveURL(/\/setup$/);
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("관리자");
    await page.getByLabel("이메일").fill("Admin@Example.COM");
    await page.getByLabel("비밀번호").fill("supersecret1");
    await page.getByLabel("워크스페이스 이름").fill("Share Vue");
    await page.getByLabel("주소(영문)").fill("acme");
    await page.getByRole("button", { name: "시작하기" }).click();
  } else {
    await page.goto("/login");
    await page.getByLabel("이메일").fill("Admin@Example.COM");
    await page.getByLabel("비밀번호").fill("supersecret1");
    await page.getByRole("button", { name: "로그인", exact: true }).click();
  }
  await expect(page).toHaveURL(/\/$/);
  const response = await page.request.get("/api/v1/me/workspaces");
  expect(response.ok()).toBe(true);
  return (await response.json()).items.find((item: { slug: string }) => item.slug === "acme").id;
}

async function expectReader(page: Page): Promise<void> {
  await expect(page.locator('[data-public-share="vue"]')).toBeVisible();
  expect(await page.locator("#root").evaluate((root) => Boolean((root as HTMLElement & { __vue_app__?: unknown }).__vue_app__))).toBe(true);
  await expect(page.locator("[contenteditable], .ProseMirror, .fvoci-editor")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "저장", exact: true })).toHaveCount(0);
}

test("public Vue document URL renders readonly content, hands off attachments and denies refreshed expiry/revoke", async ({ page, browser }) => {
  test.setTimeout(90_000);
  const ws = await setup(page);
  const create = async (title: string, parentId: string | null = null) => {
    const response = await page.request.post(`/api/v1/workspaces/${ws}/documents`, { data: { title, parentId } });
    expect(response.status()).toBe(201);
    return (await response.json()).id as string;
  };
  const root = await create("공유 한글 루트");
  const child = await create("공유 하위 문서", root);
  const outside = await create("공유 밖 비공개");
  const bytes = Buffer.from("첨부 한글 ✅", "utf8");
  const uploadRes = await page.request.post(`/api/v1/workspaces/${ws}/documents/${root}/uploads`, { data: { name: "memo.txt", sizeBytes: bytes.length } });
  expect(uploadRes.ok()).toBe(true);
  const upload = await uploadRes.json();
  const parts = [];
  for (const part of upload.parts) {
    const put = await page.request.put(part.url, { data: bytes, headers: { "content-type": "application/octet-stream" } });
    expect(put.ok()).toBe(true);
    parts.push({ partNumber: part.partNumber, etag: put.headers()["etag"] });
  }
  expect((await page.request.post(`/api/v1/workspaces/${ws}/attachments/${upload.attachmentId}/complete`, { data: { parts } })).ok()).toBe(true);
  const shareRes = await page.request.post(`/api/v1/workspaces/${ws}/documents/${root}/share-links`, { data: { expiresInDays: 7 } });
  expect(shareRes.status()).toBe(201);
  const share = await shareRes.json();
  const sharePath = new URL(share.url).pathname;
  const token = sharePath.split("/")[2];
  const viewerPath = `${sharePath}/attachments/${upload.attachmentId}/view`;
  const content = { type: "doc", content: [
    { type: "paragraph", content: [{ type: "text", text: "본문 한글 ✅ <script>window.shareXss=1</script>" }] },
    { type: "paragraph", content: [{ type: "text", text: "위험 링크", marks: [{ type: "link", attrs: { href: "javascript:alert(1)" } }] }] },
    { type: "paragraph", content: [{ type: "text", text: "첨부 열기", marks: [{ type: "link", attrs: { href: viewerPath } }] }] },
  ] };
  seedContent(root, content);
  seedContent(child, { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "하위 본문" }] }] });
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  const csp = watchCspViolations(reader);
  const requests: string[] = [];
  const sockets: string[] = [];
  reader.on("request", request => { const path = new URL(request.url()).pathname; if (path.startsWith("/api/")) requests.push(path); });
  reader.on("websocket", socket => sockets.push(socket.url()));
  await reader.goto(sharePath);
  await expectReader(reader);
  const body = reader.getByTestId("share-body");
  await expect(body).toContainText("본문 한글 ✅");
  await expect(body.locator("script, iframe, [onclick]")).toHaveCount(0);
  await expect(body.locator('a[href^="javascript:"]')).toHaveCount(0);
  const link = body.getByRole("link", { name: "첨부 열기" });
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
  const popupEvent = reader.waitForEvent("popup");
  await link.click();
  const viewer = await popupEvent;
  await expect(viewer).toHaveURL(new RegExp(`${viewerPath}$`));
  await expect(viewer.locator("pre.attachment-viewer__text")).toHaveText("첨부 한글 ✅");
  await expect(viewer.locator("#root")).toHaveAttribute("data-v-app", "");
  await viewer.close();
  await reader.getByRole("navigation").getByRole("button", { name: "공유 하위 문서" }).click();
  await expect(body).toHaveText("하위 본문");
  await reader.reload();
  await expectReader(reader);
  await expect(body).toContainText("본문 한글 ✅");
  expect((await reader.request.get(`/api/v1/share/${token}/documents/${outside}?format=fragment`)).status()).toBe(404);

  // The API is still real Rust/PG while cached body data exists in the Vue query client.
  fixtureSql(`UPDATE fvoci.share_links SET expires_at = now() - interval '1 second' WHERE id = ${uuid(share.id)};`);
  await reader.getByRole("button", { name: "다시 시도", exact: true }).click();
  await expect(reader.getByRole("alert")).toHaveText("공유 링크가 만료되었습니다");
  await expect(body).toHaveCount(0);
  await reader.reload();
  await expect(body).toHaveCount(0);
  await expect(reader.getByRole("alert")).toBeVisible();
  fixtureSql(`UPDATE fvoci.share_links SET expires_at = now() + interval '1 day' WHERE id = ${uuid(share.id)};`);
  await reader.getByRole("button", { name: "다시 시도", exact: true }).click();
  await expect(body).toContainText("본문 한글 ✅");
  expect((await page.request.delete(`/api/v1/workspaces/${ws}/share-links/${share.id}`)).ok()).toBe(true);
  // Refetch just the body by leaving and returning to the root: denial must remove cached content.
  await reader.getByRole("navigation").getByRole("button", { name: "공유 하위 문서" }).click();
  await expect(reader.getByRole("alert")).toBeVisible();
  await expect(body).toHaveCount(0);
  await reader.reload();
  await expect(reader.getByRole("alert")).toBeVisible();
  await expect(body).toHaveCount(0);
  expect((await reader.request.get(`/api/v1/share/${token}/attachments/${upload.attachmentId}/download`)).status()).toBe(404);
  expect(requests.length).toBeGreaterThan(0);
  expect(requests.every(path => path.startsWith("/api/v1/share/"))).toBe(true);
  expect(sockets).toEqual([]);
  expect(await anon.cookies()).toEqual([]);
  expect(csp).toEqual([]);
  await anon.close();
});

test("public Vue project share presents task search excerpts and rechecks archived and deleted scope", async ({ page, browser }) => {
  test.setTimeout(90_000);
  const ws = await setup(page);
  const projectRes = await page.request.post(`/api/v1/workspaces/${ws}/projects`, { data: { key: "SHR", name: "비공개 프로젝트 공유", visibility: "private" } });
  expect(projectRes.status()).toBe(201);
  const project = await projectRes.json();
  const taskRes = await page.request.post(`/api/v1/workspaces/${ws}/projects/${project.id}/tasks`, { data: { title: "sharemarker 공개 과제" } });
  expect(taskRes.status()).toBe(201);
  const task = await taskRes.json();
  seedContent(task.id, { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "sharemarker <script>태스크 한글 ✅</script>" }] }] }, "tasks");
  const shareRes = await page.request.post(`/api/v1/workspaces/${ws}/share-links`, { data: { projectId: project.id, expiresInDays: 7 } });
  expect(shareRes.status()).toBe(201);
  const share = await shareRes.json();
  const sharePath = new URL(share.url).pathname;
  const token = sharePath.split("/")[2];
  const anon = await browser.newContext();
  const reader = await anon.newPage();
  const csp = watchCspViolations(reader);
  await reader.goto(sharePath);
  await expectReader(reader);
  // Wait for the normal outbox indexing; index recall never substitutes for PG authorization.
  await expect.poll(async () => (await (await reader.request.get(`/api/v1/share/${token}/search?q=sharemarker`)).json()).items?.some((item: { id: string }) => item.id === task.id), { timeout: 30_000 }).toBe(true);
  const search = reader.getByRole("searchbox", { name: "검색" });
  await search.fill("sharemarker");
  const results = reader.getByTestId("share-search-results");
  await expect(results).toContainText("sharemarker 공개 과제");
  await expect(results).toContainText("태스크 한글 ✅");
  await expect(results.locator("script, iframe, [contenteditable]")).toHaveCount(0);
  await expect(results.getByRole("link")).toHaveCount(0);
  fixtureSql(`UPDATE fvoci.tasks SET archived_at = now() WHERE id = ${uuid(task.id)};`);
  await search.fill("");
  await expect(results).toHaveCount(0);
  await search.fill("sharemarker ");
  await expect(reader.getByText("결과가 없습니다", { exact: true })).toBeVisible();
  await reader.reload();
  await expectReader(reader);
  await reader.getByRole("searchbox", { name: "검색" }).fill("sharemarker");
  await expect(reader.getByText("결과가 없습니다", { exact: true })).toBeVisible();
  fixtureSql(`UPDATE fvoci.projects SET deleted_at = now() WHERE id = ${uuid(project.id)};`);
  await reader.getByRole("button", { name: "다시 시도", exact: true }).click();
  await expect(reader.getByRole("alert")).toBeVisible();
  await expect(reader.getByTestId("share-body")).toHaveCount(0);
  expect(csp).toEqual([]);
  await anon.close();
});
