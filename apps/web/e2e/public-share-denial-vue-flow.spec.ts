import { execFileSync } from "node:child_process";
import { expect, test } from "@playwright/test";
import { watchCspViolations } from "./helpers";

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

test("observed body denial gates cached share tree, heading and snippets; full refresh recovers a live root", async ({ page, browser }) => {
  test.setTimeout(90_000);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/);
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("관리자");
  await page.getByLabel("이메일").fill("Admin@Example.COM");
  await page.getByLabel("비밀번호").fill("supersecret1");
  await page.getByLabel("워크스페이스 이름").fill("Share denial");
  await page.getByLabel("주소(영문)").fill("acme");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const workspaces = await page.request.get("/api/v1/me/workspaces");
  expect(workspaces.ok()).toBe(true);
  const ws = (await workspaces.json()).items.find((item: { slug: string }) => item.slug === "acme").id;
  const create = async (title: string, parentId: string | null = null) => {
    const response = await page.request.post(`/api/v1/workspaces/${ws}/documents`, { data: { title, parentId } });
    expect(response.status()).toBe(201);
    return (await response.json()).id as string;
  };

  for (const scenario of ["revoked", "expired", "moved"] as const) {
    await test.step(scenario, async () => {
      const q = `cachemarker${scenario}`;
      const rootTitle = `${q} root`;
      const childTitle = `${q} child`;
      const secret = `SECRET-${scenario} 한글 ✅`;
      const root = await create(rootTitle);
      const child = await create(childTitle, root);
      await create(`${q} sibling`, root);
      const outside = scenario === "moved" ? await create("공유 밖 문서") : null;
      for (const [id, text] of [[root, "현재 허용된 루트 본문"], [child, `${q} ${secret}`]]) {
        const content = JSON.stringify({ type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text }] }] });
        // Match the persistence projection: document search hydrates `text`, not content_json.
        fixtureSql(`UPDATE fvoci.documents SET content_json = '${content}'::jsonb, text = '${text}' WHERE id = ${uuid(id)};`);
      }
      // Wait for normal index recall using the owner; no polling of the public rate limit.
      await expect.poll(async () => {
        const response = await page.request.get(`/api/v1/workspaces/${ws}/search?q=${q}`);
        expect(response.ok()).toBe(true);
        return (await response.json()).items.some((item: { id: string }) => item.id === child);
      }, { timeout: 30_000 }).toBe(true);
      const shareRes = await page.request.post(`/api/v1/workspaces/${ws}/documents/${root}/share-links`, { data: { expiresInDays: 7 } });
      expect(shareRes.status()).toBe(201);
      const share = await shareRes.json();
      const path = new URL(share.url).pathname;
      const token = path.split("/")[2];
      const anon = await browser.newContext();
      try {
        const reader = await anon.newPage();
        const csp = watchCspViolations(reader);
        const apiPaths: string[] = [];
        reader.on("request", request => {
          const apiPath = new URL(request.url()).pathname;
          if (apiPath.startsWith("/api/")) apiPaths.push(apiPath);
        });
        await reader.goto(path);
        await expect(reader.locator("#root")).toHaveAttribute("data-v-app", "");
        await expect(reader.getByTestId("share-body")).toHaveText("현재 허용된 루트 본문");
        const tree = reader.getByRole("navigation", { name: "문서" });
        await tree.getByRole("button", { name: childTitle, exact: true }).click();
        await expect(reader.getByTestId("share-body")).toContainText(secret);
        await tree.getByRole("button", { name: rootTitle, exact: true }).click();
        await expect(reader.getByTestId("share-body")).toHaveText("현재 허용된 루트 본문");
        await reader.getByRole("searchbox", { name: "검색" }).fill(q);
        await expect(reader.getByTestId("share-search-results")).toContainText(secret);
        await expect(tree.getByRole("button", { name: childTitle, exact: true })).toBeVisible();

        if (scenario === "revoked") {
          expect((await page.request.delete(`/api/v1/workspaces/${ws}/share-links/${share.id}`)).ok()).toBe(true);
        } else if (scenario === "expired") {
          fixtureSql(`UPDATE fvoci.share_links SET expires_at = now() - interval '1 second' WHERE id = ${uuid(share.id)};`);
        } else {
          const moved = await page.request.post(`/api/v1/workspaces/${ws}/documents/${child}/move`, { data: { newParentId: outside } });
          expect(moved.status(), await moved.text()).toBe(200);
        }
        // Only the selected body's request observes denial; successful tree/search are still cached.
        const denied = reader.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/share/${token}/documents/${child}`);
        await tree.getByRole("button", { name: childTitle, exact: true }).click();
        expect((await denied).status()).toBe(404);
        await expect(reader.getByRole("alert")).toHaveText("공유 링크가 만료되었습니다");
        await expect(reader.getByTestId("share-body")).toHaveCount(0);
        await expect(tree).toHaveCount(0);
        await expect(reader.getByRole("heading", { level: 1 })).toHaveCount(0);
        await expect(reader.getByTestId("share-search-results")).toHaveCount(0);
        await expect(reader.getByText(childTitle, { exact: true })).toHaveCount(0);
        await expect(reader.getByText(secret, { exact: false })).toHaveCount(0);

        // Full retry keeps invalid tokens gated, but a moved child can return to the authorized root.
        if (scenario === "expired") {
          const retryMeta = reader.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/share/${token}`);
          await reader.getByRole("button", { name: "다시 시도", exact: true }).click();
          expect((await retryMeta).status()).toBe(404);
          await expect(reader.getByRole("button", { name: "다시 시도", exact: true })).toBeEnabled();
          await expect(reader.getByRole("alert")).toBeVisible();
          await expect(tree).toHaveCount(0);
          fixtureSql(`UPDATE fvoci.share_links SET expires_at = now() + interval '1 day' WHERE id = ${uuid(share.id)};`);
        }
        const refreshedMeta = reader.waitForResponse(response => new URL(response.url()).pathname === `/api/v1/share/${token}`);
        await reader.getByRole("button", { name: "다시 시도", exact: true }).click();
        expect((await refreshedMeta).status()).toBe(scenario === "revoked" ? 404 : 200);
        if (scenario === "revoked") {
          await expect(reader.getByRole("button", { name: "다시 시도", exact: true })).toBeEnabled();
          await expect(reader.getByRole("alert")).toBeVisible();
          await expect(tree).toHaveCount(0);
          await expect(reader.getByTestId("share-search-results")).toHaveCount(0);
        } else {
          await expect(reader.getByRole("heading", { level: 1 })).toHaveText(rootTitle);
          await expect(reader.getByTestId("share-body")).toHaveText("현재 허용된 루트 본문");
          await expect(reader.getByRole("alert")).toHaveCount(0);
          await expect(reader.getByRole("searchbox", { name: "검색" })).toHaveValue("");
          if (scenario === "moved") {
            await expect(tree.getByRole("button", { name: childTitle, exact: true })).toHaveCount(0);
            await reader.getByRole("searchbox", { name: "검색" }).fill(q);
            await expect(reader.getByTestId("share-search-results")).toBeVisible();
            await expect(reader.getByTestId("share-search-results")).not.toContainText(secret);
            await expect(reader.getByTestId("share-search-results").getByRole("button", { name: childTitle, exact: true })).toHaveCount(0);
          }
        }
        expect(apiPaths.every(apiPath => apiPath.startsWith("/api/v1/share/"))).toBe(true);
        expect(await anon.cookies()).toEqual([]);
        expect(csp).toEqual([]);
      } finally {
        await anon.close();
      }
    });
  }
});
