import { expect, test } from "@playwright/test";
import { createE2eUser, login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
};

const member = {
  email: "wiki-member@example.com",
  password: "memberpass1",
  givenName: "위키",
  familyName: "멤버",
};

const guest = {
  email: "wiki-guest@example.com",
  password: "guestpass1",
  givenName: "게스트",
  familyName: "위키",
};

async function workspaceId(page: import("@playwright/test").Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

test("member creates wiki document and edits title metadata", async ({ page }) => {
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: "acme",
    membershipRole: "member",
  });

  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("현재 역할: 멤버")).toBeVisible();

  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/WIKI-\d+$/);
  await expect(page.getByLabel("문서 제목")).toHaveValue("제목 없음");

  const renamed = "연구 노트";
  await page.getByLabel("문서 제목").fill(renamed);
  await page.getByLabel("문서 제목").blur();
  await expect(page.getByLabel("문서 제목")).toHaveValue(renamed);

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: renamed })).toBeVisible();
});

test("document metadata supports icon set/clear and status changes", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "연구 노트" }).click();

  await page.getByLabel("아이콘").fill("📚");
  await page.getByLabel("아이콘").blur();
  await expect(page.getByLabel("아이콘")).toHaveValue("📚");

  await page.getByLabel("문서 상태").selectOption("published");
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");

  await page.getByLabel("아이콘").fill("");
  await page.getByLabel("아이콘").blur();
  await expect(page.getByLabel("아이콘")).toHaveValue("");
});

test("document metadata save failure keeps the previous status", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await page.getByRole("link", { name: "연구 노트" }).click();
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");

  await page.route("**/api/v1/workspaces/*/documents/*", async (route) => {
    if (route.request().method() === "PATCH") {
      await route.abort("connectionfailed");
      return;
    }
    await route.continue();
  });

  await page.getByLabel("문서 상태").selectOption("archived");
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByLabel("문서 상태")).toHaveValue("published");
});

test("member create transport failure shows an error without navigation", async ({ page }) => {
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await page.route("**/api/v1/workspaces/*/documents", async (route) => {
    if (route.request().method() === "POST") {
      await route.abort("connectionfailed");
      return;
    }
    await route.continue();
  });
  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page.getByRole("alert")).toContainText("연결을 확인하고 다시 시도해 주세요");
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
});

test("nested wiki tree renders more than two levels", async ({ page }) => {
  await login(page, admin.email, admin.password);
  const id = await workspaceId(page, "acme");

  const rootRes = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: null, title: "루트 문서" },
  });
  expect(rootRes.ok()).toBe(true);
  const root = await rootRes.json();

  const childRes = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: root.id, title: "중간 문서" },
  });
  expect(childRes.ok()).toBe(true);
  const child = await childRes.json();

  const grandchildRes = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: child.id, title: "하위 문서" },
  });
  expect(grandchildRes.ok()).toBe(true);

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "루트 문서" })).toBeVisible();
  await expect(page.getByRole("link", { name: "중간 문서" })).toBeVisible();
  await expect(page.getByRole("link", { name: "하위 문서" })).toBeVisible();
});

test("admin sees member document and guest cannot read wiki", async ({ page }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "연구 노트" })).toBeVisible();

  const id = await workspaceId(page, "acme");
  const treeRes = await page.request.get(`/api/v1/workspaces/${id}/tree`);
  expect(treeRes.ok()).toBe(true);
  const treeBody = await treeRes.json();
  const document = treeBody.items.find((item: { title: string }) => item.title === "연구 노트");
  expect(document).toBeTruthy();

  createE2eUser(guest.email, guest.password, guest.givenName, {
    familyName: guest.familyName,
    workspaceSlug: "acme",
    membershipRole: "guest",
  });

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, guest.email, guest.password);

  const guestTreeRes = await page.request.get(`/api/v1/workspaces/${id}/tree`);
  expect(guestTreeRes.ok()).toBe(true);
  expect((await guestTreeRes.json()).items).toEqual([]);

  const guestDocRes = await page.request.get(
    `/api/v1/workspaces/${id}/documents/${document.id}`,
  );
  expect(guestDocRes.status()).toBe(404);
  expect((await guestDocRes.json()).code).toBe("not_found");

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByText("현재 역할: 게스트")).toBeVisible();
  await expect(page.getByRole("button", { name: "새 문서" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "연구 노트" })).toHaveCount(0);
  await expect(page.getByText("문서가 없습니다")).toBeVisible();

  await page.goto(`/w/acme/WIKI-${document.number}`);
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
});
