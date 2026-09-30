import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { createE2eUser, login, logout, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });

const owner = { email: "workspace-settings-owner@example.com", password: "settingspass1" };
let workspaceId: string;

async function openSettings(page: Page, suffix = ""): Promise<void> {
  await page.goto(`/w/settings-vue/settings${suffix}`);
  await expect(page.locator("#root[data-v-app]")).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/w/settings-vue/settings${suffix}$`));
}

test("Vue settings commit identity, groups, tokens, holidays, preferences and recovered import/export", async ({ page }) => {
  test.setTimeout(120_000);
  const csp = watchCspViolations(page);
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(err.message));
  await page.goto("/setup");
  await page.getByLabel("성").fill("김");
  await page.getByLabel("이름", { exact: true }).fill("설정");
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill("Settings Vue");
  await page.getByLabel("주소(영문)").fill("settings-vue");
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
  const workspaces = await page.request.get("/api/v1/me/workspaces");
  workspaceId = (await workspaces.json()).items.find((item: { slug: string }) => item.slug === "settings-vue").id;

  const metadataUrl = `**/api/v1/workspaces/${workspaceId}`;
  await page.route(metadataUrl, (route) => route.abort("failed"));
  await openSettings(page);
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(/\/settings$/);
  await page.unroute(metadataUrl);
  await page.getByRole("button", { name: "다시 시도" }).click();
  await page.getByLabel("워크스페이스 이름").fill("Settings renamed");
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "저장" })).toBeVisible();
  expect((await (await page.request.get(`/api/v1/workspaces/${workspaceId}`)).json()).name).toBe("Settings renamed");

  const groups = page.locator("details").filter({ has: page.locator("summary", { hasText: /^그룹$/ }) });
  await groups.locator("summary").click();
  await groups.getByLabel("그룹 이름").fill("Settings group");
  await groups.getByRole("button", { name: "만들기", exact: true }).click();
  await expect(groups.getByRole("button", { name: "Settings group", exact: true })).toBeVisible();
  await groups.getByLabel("그룹 이름").fill("x".repeat(101));
  await groups.getByRole("button", { name: "만들기", exact: true }).click();
  await expect(groups.getByRole("alert")).toBeVisible();
  await groups.getByLabel("그룹 이름").fill("Recovered group");
  await groups.getByRole("button", { name: "만들기", exact: true }).click();
  await expect(groups.getByRole("button", { name: "Recovered group", exact: true })).toBeVisible();

  const tokens = page.locator("details").filter({ has: page.locator("summary", { hasText: /^토큰$/ }) });
  await tokens.locator("summary").click();
  await tokens.getByRole("textbox", { name: "이름", exact: true }).fill("Read-only settings token");
  await tokens.getByLabel("문서 조회").check();
  await tokens.getByRole("button", { name: "발급" }).click();
  const secretField = tokens.getByRole("textbox", { name: "이 토큰 값은 지금만 보입니다" });
  await expect(secretField).toBeVisible();
  const secret = await secretField.inputValue();
  const auth = { Authorization: `Bearer ${secret}` };
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/documents`, { headers: auth })).ok()).toBe(true);
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, { headers: auth, data: { title: "Denied", parentId: null } })).ok()).toBe(false);
  const listedTokens = await (await page.request.get(`/api/v1/workspaces/${workspaceId}/api-tokens`)).json();
  expect(JSON.stringify(listedTokens)).not.toContain(secret);
  await page.reload();
  await tokens.locator("summary").click();
  await expect(secretField).toHaveCount(0);
  await tokens.getByRole("button", { name: "폐기" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "폐기" }).click();
  await expect(tokens.getByText("토큰이 없습니다")).toBeVisible();
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/documents`, { headers: auth })).ok()).toBe(false);

  const calendar = page.locator("details").filter({ has: page.locator("summary", { hasText: /달력 구독/ }) });
  await calendar.locator("summary").click();
  await calendar.getByLabel("공휴일 날짜").fill("2026-10-09");
  await calendar.getByRole("button", { name: "공휴일 추가" }).click();
  await expect(calendar.locator("time")).toHaveText("2026-10-09");
  await calendar.getByRole("button", { name: "2026-10-09 삭제" }).click();
  await expect(calendar.locator("time")).toHaveCount(0);
  const prefsUrl = `**/api/v1/workspaces/${workspaceId}/notification-prefs`;
  const initialPrefs = await (await page.request.get(`/api/v1/workspaces/${workspaceId}/notification-prefs`)).json();
  await page.route(prefsUrl, (route) => route.request().method() === "PUT" ? route.abort("failed") : route.continue());
  const prefsSection = page.locator("section").filter({ has: page.getByRole("heading", { name: "알림", exact: true }) });
  await page.getByLabel("인앱 알림", { exact: true }).click();
  await expect(prefsSection.getByRole("alert")).toBeVisible();
  await expect(page.getByLabel("인앱 알림", { exact: true })).toBeChecked({ checked: initialPrefs.inApp });
  await page.unroute(prefsUrl);
  await page.getByLabel("메일 다이제스트").click();
  await expect.poll(async () => (await (await page.request.get(`/api/v1/workspaces/${workspaceId}/notification-prefs`)).json()).mailDigest).toBe(true);

  // Only the failed request is intercepted. Recovery downloads actual Rust output.
  const exportUrl = `**/api/v1/workspaces/${workspaceId}/export`;
  await page.route(exportUrl, (route) => route.abort("failed"));
  const exportSection = page.locator("section").filter({ has: page.getByRole("heading", { name: "워크스페이스 내보내기", exact: true }) });
  await exportSection.getByRole("button").click();
  await expect(exportSection.getByRole("alert")).toBeVisible();
  await page.unroute(exportUrl);
  const downloadPromise = page.waitForEvent("download");
  await exportSection.getByRole("button").click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("fvoci-workspace.zip");
  await expect(exportSection.getByRole("alert")).toHaveCount(0);

  await page.locator('input[type="file"]').setInputFiles({ name: "invalid.zip", mimeType: "application/zip", buffer: Buffer.from("not a zip") });
  const importSection = page.locator("section").filter({ has: page.getByRole("heading", { name: "가져올 형식", exact: true }) });
  await expect(importSection.getByRole("alert")).toBeVisible();
  await expect(importSection.getByRole("status")).toHaveCount(0);
  await page.locator('input[type="file"]').setInputFiles(path.resolve(import.meta.dirname, "fixtures/markdown-import.zip"));
  await expect(importSection.getByRole("status")).toHaveText("가져오기를 시작했습니다", { timeout: 30_000 });
  const documents = await (await page.request.get(`/api/v1/workspaces/${workspaceId}/documents`)).json();
  expect(documents.items.some((item: { title: string }) => item.title === "e2e-note")).toBe(true);

  const sso = page.locator("details").filter({ has: page.locator("summary", { hasText: /^싱글 사인온$/ }) });
  await sso.locator("summary").click();
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/oidc`)).status()).toBe(404);
  await expect(sso.locator("form")).toHaveCount(0);
  await expect(page.getByTestId("workspace-events")).toContainText("workspace");
  expect(csp).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("actual Vue tags and templates URLs persist edits, apply documents, and enforce member restrictions", async ({ page }) => {
  test.setTimeout(90_000);
  const csp = watchCspViolations(page);
  await login(page, owner.email, owner.password);
  await openSettings(page, "/document-tags");
  await page.getByLabel("이름", { exact: true }).fill("Settings tag");
  await page.getByLabel("색", { exact: true }).selectOption("blue");
  await page.getByRole("button", { name: "만들기", exact: true }).click();
  const row = page.getByTestId("document-tag-row-Settings tag");
  await expect(row).toBeVisible();
  await row.getByRole("textbox").fill("Renamed settings tag");
  await row.getByRole("textbox").press("Enter");
  await expect(page.getByTestId("document-tag-row-Renamed settings tag")).toBeVisible();
  await page.reload();
  await expect(page.getByTestId("document-tag-row-Renamed settings tag")).toBeVisible();
  await openSettings(page, "/templates");
  await page.getByLabel("제목").fill("Settings document template");
  await page.getByRole("button", { name: "추가", exact: true }).click();
  const template = page.getByRole("row", { name: /Settings document template/ });
  await expect(template).toBeVisible();
  await template.getByRole("button", { name: "적용" }).click();
  await expect(page).toHaveURL(/\/w\/settings-vue\/WIKI-\d+$/);
  await expect(page.getByLabel("문서 제목")).toHaveValue("Settings document template");

  const projectResponse = await page.request.post(`/api/v1/workspaces/${workspaceId}/projects`, { data: { key: "REC", name: "Recovered settings project", visibility: "workspace" } });
  expect(projectResponse.status()).toBe(201);
  const project = await projectResponse.json();
  expect((await page.request.delete(`/api/v1/workspaces/${workspaceId}/projects/${project.id}`)).ok()).toBe(true);
  await openSettings(page);
  await page.getByTestId("deleted-projects").getByRole("button", { name: "복원 Recovered settings project" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "복원", exact: true }).click();
  await expect(page.getByTestId("deleted-projects")).toHaveCount(0);
  const activeProjects = await (await page.request.get(`/api/v1/workspaces/${workspaceId}/projects`)).json();
  expect(activeProjects.items.some((item: { id: string }) => item.id === project.id)).toBe(true);

  await logout(page);
  createE2eUser("settings-member@example.com", "memberpass1", "멤버", { familyName: "이", workspaceSlug: "settings-vue", membershipRole: "member" });
  await login(page, "settings-member@example.com", "memberpass1");
  await openSettings(page);
  await expect(page.getByText("설정을 변경하려면 관리자 권한이 필요합니다")).toBeVisible();
  await expect(page.getByRole("button", { name: "워크스페이스 내보내기" })).toHaveCount(0);
  await expect(page.locator("summary").filter({ hasText: /^(토큰|웹훅|워크스페이스 삭제)$/ })).toHaveCount(0);
  expect((await page.request.get(`/api/v1/workspaces/${workspaceId}/export`)).status()).toBe(404);
  expect((await page.request.post(`/api/v1/workspaces/${workspaceId}/groups`, { data: { name: "Denied group" } })).ok()).toBe(false);
  await openSettings(page, "/document-tags");
  await expect(page.getByTestId("document-tag-row-Renamed settings tag").getByRole("textbox")).toHaveCount(0);
  expect(csp).toEqual([]);
});
