import { expect, test } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "lifecycle-member@example.com",
  password: "memberpass1",
  givenName: "멤버",
  familyName: "이",
};

test("home counts and owner deletes a team workspace", async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });

  await page.getByLabel("성").fill(admin.familyName);
  await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
  await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("heading", { name: "나의 대시보드" })).toBeVisible();
  await expect(page.getByText("문서 0개")).toBeVisible();
  await expect(page.getByText("담당 태스크 0개")).toBeVisible();

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const acme = workspacesBody.items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  expect(acme).toBeTruthy();
  const created = await page.request.post(`/api/v1/workspaces/${acme.id}/documents`, {
    data: { title: "수명주기 문서", parentId: null },
  });
  expect(created.ok()).toBe(true);

  await page.reload();
  await expect(page.getByText("문서 1개")).toBeVisible();

  await page.getByRole("button", { name: "만들기" }).click();
  await page.getByLabel("워크스페이스 이름").fill("Beta 팀");
  await page.getByLabel("주소(영문)").fill("beta-team");
  await page.getByRole("dialog").getByRole("button", { name: "만들기" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Beta 팀" })).toBeVisible();

  await page.getByRole("link", { name: "Beta 팀" }).click();
  await expect(page).toHaveURL(/\/w\/beta-team\/wiki$/);
  await page.goto("/w/beta-team/settings");
  await page.locator("summary").filter({ hasText: /^워크스페이스 삭제$/ }).click();
  await page.getByLabel("확인을 위해 주소(영문)를 입력하세요.").fill("beta-team");
  await page.getByRole("button", { name: "워크스페이스 삭제" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("link", { name: "Beta 팀" })).toHaveCount(0);
  await expect(page.getByText(admin.workspaceName)).toBeVisible();

  await logout(page);
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
  await login(page, member.email, member.password);
  await page.goto("/w/acme/settings");
  await expect(page.getByText("설정을 변경하려면 관리자 권한이 필요합니다")).toBeVisible();
  await expect(page.locator("summary").filter({ hasText: /^워크스페이스 삭제$/ })).toHaveCount(0);
});
