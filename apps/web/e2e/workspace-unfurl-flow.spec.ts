import { expect, test, type Page } from "@playwright/test";
import { readJson, flowSchemas, login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "unfurl",
  workspaceName: "Unfurl Preview",
};

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

test("editor URL embed shows authenticated unfurl card", async ({ page }) => {
  await page.route("**/api/v1/workspaces/*/unfurl**", async (route) => {
    const url = new URL(route.request().url());
    const target = url.searchParams.get("url") ?? "";
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        kind: "og",
        url: target,
        title: "미리보기 제목",
        description: "미리보기 설명",
        imageUrl: null,
        state: null,
        number: null,
        owner: null,
        repo: null,
      }),
    });
  });

  await ensureSetup(page);
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const fixtureValue1 = (await readJson(workspacesRes, flowSchemas.workspaces)).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  if (fixtureValue1 === undefined)
    throw new Error(
      "Missing fixture value: (await readJson(workspacesRes, flowSchemas.workspaces)).items.find(\n    (item: { slug: string }) => item.slug === admin.workspaceSlug,\n  )",
    );
  const wsId = fixtureValue1.id;
  const created = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title: "링크 미리보기" },
  });
  expect(created.status()).toBe(201);
  const document = (await readJson(created, flowSchemas.document)) as {
    id: string;
    number: number;
  };
  await page.goto(`/w/${admin.workspaceSlug}/WIKI-${String(document.number)}`);
  const editor = page.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible({ timeout: 15000 });
  await editor.click();
  await editor.pressSequentially("/https://example.com/preview", { delay: 20 });
  await page.getByRole("option", { name: "URL 임베드" }).click();
  await expect(page.getByText("미리보기 제목")).toBeVisible();
  await expect(page.getByText("미리보기 설명")).toBeVisible();
  await expect(page.getByRole("link", { name: "링크 열기" })).toBeVisible();
});
