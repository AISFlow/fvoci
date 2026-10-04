import { expect, test } from "@playwright/test";
import { readJson, flowSchemas, createE2eUser, login } from "./helpers";

test.describe.configure({ mode: "serial" });

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
  givenName: "라이프",
  familyName: "사이클",
};

async function workspaceId(page: import("@playwright/test").Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await readJson(workspacesRes, flowSchemas.workspaces)).items.find(
    (item: { slug: string }) => item.slug === slug,
  );
  if (workspace === undefined) throw new Error("Missing fixture value: workspace");
  expect(workspace).toBeTruthy();
  return workspace.id;
}

test("instance setup", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15000 });
  await page.getByLabel("성").fill(admin.familyName);
  await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
  await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
});

test("rename, move, trash, and restore wiki documents", async ({ page }) => {
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

  const wsId = await workspaceId(page, "acme");
  const treeBefore = await page.request.get(`/api/v1/workspaces/${wsId}/tree`);
  const doc = (await readJson(treeBefore, flowSchemas.documents)).items.at(-1);
  if (doc === undefined) throw new Error("Missing fixture value: doc");
  expect(doc).toBeTruthy();

  const renamed = page.waitForResponse(
    (response) =>
      response.request().method() === "PATCH" &&
      response.url().includes(`/documents/${doc.id}`) &&
      response.ok(),
  );
  await page.getByLabel("문서 제목").fill("라이프사이클 문서");
  await page.getByLabel("문서 제목").blur();
  await renamed;
  await expect(page.getByLabel("문서 제목")).toHaveValue("라이프사이클 문서");

  const parentRes = await page.request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { commandId: crypto.randomUUID(), parentId: null, title: "이동 대상 부모" },
  });
  expect(parentRes.ok()).toBe(true);
  const targetParent = await readJson(parentRes, flowSchemas.document);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "이동 대상 부모" })).toBeVisible();
  await page.goto(`/w/acme/WIKI-${String(doc.number)}`);
  await page.getByRole("button", { name: "문서 옵션", exact: true }).click();
  const moveSelect = page.getByLabel("새 위치(부모 문서)");
  await expect(moveSelect).toBeVisible();
  await expect(moveSelect.locator('option[value=""]')).toHaveCount(1);
  await expect(moveSelect.locator(`option[value="${targetParent.id}"]`)).toHaveCount(1, {
    timeout: 15000,
  });
  await moveSelect.selectOption(targetParent.id);
  const moveResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && response.url().includes("/move") && response.ok(),
  );
  await page.getByRole("button", { name: "이동", exact: true }).click();
  await moveResponse;

  const movedTree = await page.request.get(`/api/v1/workspaces/${wsId}/tree`);
  const moved = (await readJson(movedTree, flowSchemas.documents)).items.find(
    (item: { id: string }) => item.id === doc.id,
  );
  expect(moved?.parentId).toBe(targetParent.id);

  await page.evaluate(() => {
    window.confirm = () => true;
  });
  const trashResponse = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && response.url().includes("/trash") && response.ok(),
  );
  await page.getByRole("button", { name: "휴지통으로 이동" }).click();
  await trashResponse;
  await expect(page).toHaveURL(/\/w\/acme\/trash$/, { timeout: 15000 });
  const trashList = await page.request.get(`/api/v1/workspaces/${wsId}/trash`);
  expect(trashList.ok()).toBe(true);
  expect(
    (await readJson(trashList, flowSchemas.trash)).items.some(
      (item: { title: string }) => item.title === "라이프사이클 문서",
    ),
  ).toBe(true);
  await expect(page.getByText("라이프사이클 문서")).toBeVisible({ timeout: 15000 });
  await page.getByRole("button", { name: "복원 라이프사이클 문서" }).click();
  await expect(page.getByText("휴지통이 비었습니다")).toBeVisible();

  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("link", { name: "라이프사이클 문서" })).toBeVisible();
});
