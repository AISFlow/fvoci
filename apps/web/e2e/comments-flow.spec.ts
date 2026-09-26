import { expect, test } from "@playwright/test";
import { createE2eUser, login, logout } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "comments-member@example.com",
  password: "memberpass1",
  givenName: "댓글",
  familyName: "멤버",
};

test("member adds and resolves a wiki document comment", async ({ page }) => {
  test.setTimeout(90_000);

  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: owner.workspaceSlug,
    membershipRole: "member",
  });

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === owner.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  const membersRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/members`);
  expect(membersRes.ok()).toBe(true);
  const memberId = (await membersRes.json()).items.find(
    (item: { email: string }) => item.email.toLowerCase() === member.email.toLowerCase(),
  )?.userId;
  expect(memberId).toBeTruthy();
  const groupRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/groups`, {
    data: { name: "랩팀" },
  });
  expect(groupRes.status(), await groupRes.text()).toBe(201);
  const groupId = (await groupRes.json()).id;
  const addRes = await page.request.post(
    `/api/v1/workspaces/${workspace.id}/groups/${groupId}/members`,
    { data: { userId: memberId } },
  );
  expect(addRes.status(), await addRes.text()).toBe(201);
  const projRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key: "PDC", name: "문서댓글", visibility: "workspace" },
  });
  expect(projRes.status(), await projRes.text()).toBe(201);

  await logout(page);
  await login(page, member.email, member.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await page.getByRole("button", { name: "새 문서" }).click();
  await expect(page).toHaveURL(/\/w\/acme\/WIKI-\d+$/);

  const panel = page.getByTestId("document-comments");
  await expect(panel.getByRole("heading", { name: "댓글" })).toBeVisible();

  const compose = panel.locator("[data-comment-compose] textarea");
  await compose.fill("E2E 댓글입니다");
  await panel.getByRole("button", { name: "등록" }).click();
  await expect(panel.getByText("E2E 댓글입니다")).toBeVisible();

  await panel.getByRole("button", { name: "해결" }).click();
  await expect(panel.getByRole("button", { name: "다시 열기" })).toBeVisible();

  await panel.getByRole("button", { name: "반응 👍" }).click();
  await expect(panel.getByRole("button", { name: "반응 👍", pressed: true })).toContainText("1");

  await panel.getByRole("button", { name: "답글" }).click();
  const reply = panel.locator("[data-comment-reply] textarea");
  await reply.fill("답글 초안은 본문과 분리");
  await expect(compose).toHaveValue("");
  await expect(reply).toHaveValue("답글 초안은 본문과 분리");

  await page.goto("/w/acme/PDC-1");
  // Project documents open in the full editor view; its title is an input.
  await expect(page.getByLabel("문서 제목")).toHaveValue("문서댓글", { timeout: 15_000 });
  const projectPanel = page.getByTestId("document-comments");
  await expect(projectPanel.getByRole("heading", { name: "댓글" })).toBeVisible();
  const projectCompose = projectPanel.locator("[data-comment-compose] textarea");
  await expect(projectPanel.getByLabel("그룹")).toBeVisible();
  await projectPanel.getByLabel("그룹").selectOption({ label: "랩팀" });
  await expect(projectCompose).toHaveValue(/@랩팀/);
  await projectPanel.getByRole("button", { name: "등록" }).click();
  await expect(projectPanel.getByText(/@랩팀/)).toBeVisible();
});
