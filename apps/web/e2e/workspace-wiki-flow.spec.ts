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

test("member creates wiki document and edits title metadata", async ({ page }) => {
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: "acme",
    membershipRole: "member",
  });

  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();

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

test("admin sees member document and guest cannot read wiki", async ({ page }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "연구 노트" })).toBeVisible();

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === "acme");
  expect(workspace).toBeTruthy();
  const treeRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/tree`);
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

  const guestTreeRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/tree`);
  expect(guestTreeRes.ok()).toBe(true);
  expect((await guestTreeRes.json()).items).toEqual([]);

  const guestDocRes = await page.request.get(
    `/api/v1/workspaces/${workspace.id}/documents/${document.id}`,
  );
  expect(guestDocRes.status()).toBe(404);
  expect((await guestDocRes.json()).code).toBe("not_found");

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.getByRole("link", { name: "연구 노트" })).toHaveCount(0);
  await expect(page.getByText("문서가 없습니다")).toBeVisible();

  await page.goto(`/w/acme/WIKI-${document.number}`);
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
});
