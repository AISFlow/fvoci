import { expect, test } from "@playwright/test";
import { createE2eUser, login } from "./helpers";

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const member = {
  email: "member@example.com",
  password: "memberpass1",
  givenName: "멤버",
  familyName: "이",
};

const readonlyMember = {
  email: "readonly-member@example.com",
  password: "readonlypass1",
  givenName: "읽기",
  familyName: "전용",
};

test("setup → home → rename → logout → login → denied workspace", async ({ page }) => {
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
  await expect(page.getByText(admin.workspaceName)).toBeVisible();

  const me = await page.request.get("/api/v1/auth/me");
  expect(me.ok()).toBe(true);
  const meBody = await me.json();
  expect(meBody.email).toBe("admin@example.com");

  await page.getByRole("link", { name: admin.workspaceName }).click();
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
  await page.goto("/w/acme/settings");
  const renamed = "Renamed 워크스페이스";
  await page.getByLabel("워크스페이스 이름", { exact: true }).fill(renamed);
  await page.getByRole("button", { name: "저장" }).click();
  await expect(page.getByText("저장했습니다")).toBeVisible();

  await page.goto("/");
  await expect(page.getByText(renamed)).toBeVisible();

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);

  await page.getByLabel("이메일").fill("ADMIN@example.com");
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByText(renamed)).toBeVisible();
});

test("second user cannot access foreign workspace settings", async ({ page }) => {
  createE2eUser(member.email, member.password, member.givenName);

  await login(page, member.email, member.password);

  await page.goto("/w/acme/settings");
  await expect(page).toHaveURL(/\?denied=workspace$/);
  await expect(page.getByRole("alert")).toContainText("접근 권한");
  await page.getByRole("button", { name: "닫기" }).click();
  await expect(page).not.toHaveURL(/\?denied=workspace/);
});

test("instance admin creates a team workspace from home dialog", async ({ page }) => {
  await login(page, admin.email, admin.password);

  await page.getByRole("button", { name: "만들기" }).click();
  const betaName = "Beta 팀";
  const betaSlug = "beta-team";
  await page.getByLabel("워크스페이스 이름").fill(betaName);
  await page.getByLabel("주소(영문)").fill(betaSlug);
  await page.getByRole("dialog").getByRole("button", { name: "만들기" }).click();

  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByRole("link", { name: betaName })).toBeVisible();
  await page.getByRole("link", { name: betaName }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${betaSlug}/wiki$`));
  await page.goto(`/w/${betaSlug}/settings`);
  await expect(page.getByRole("heading", { name: "워크스페이스" })).toBeVisible();
  await expect(page.getByLabel("워크스페이스 이름", { exact: true })).toHaveValue(betaName);
});

test("readonly member sees read-only settings and refreshed name after admin edit", async ({ page }) => {
  createE2eUser(readonlyMember.email, readonlyMember.password, readonlyMember.givenName, {
    familyName: readonlyMember.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });

  await login(page, readonlyMember.email, readonlyMember.password);
  await page.goto("/w/acme/settings");
  await expect(page.getByText("설정을 변경하려면 관리자 권한이 필요합니다")).toBeVisible();
  await expect(page.getByRole("button", { name: "저장" })).toHaveCount(0);

  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/settings");
  const memberVisibleName = "멤버에게 보이는 이름";
  await page.getByLabel("워크스페이스 이름", { exact: true }).fill(memberVisibleName);
  await page.getByRole("button", { name: "저장" }).click();
  await expect(page.getByText("저장했습니다")).toBeVisible();

  await page.getByRole("button", { name: "로그아웃" }).click();
  await login(page, readonlyMember.email, readonlyMember.password);
  await page.goto("/w/acme/settings");
  await expect(page.getByRole("main").getByText(memberVisibleName)).toBeVisible();
  await expect(page.getByText("설정을 변경하려면 관리자 권한이 필요합니다")).toBeVisible();
});


test("logout transport failure keeps the current session visible", async ({ page }) => {
  await page.goto("/login");
  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await page.route("**/api/v1/auth/logout", (route) => route.abort("connectionfailed"));
  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(/\/$/);
  expect((await page.request.get("/api/v1/auth/me")).ok()).toBe(true);
});

test("wiki shell logout transport failure keeps the current session visible", async ({ page }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/wiki");
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await page.route("**/api/v1/auth/logout", (route) => route.abort("connectionfailed"));
  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page).toHaveURL(/\/w\/acme\/wiki$/);
  expect((await page.request.get("/api/v1/auth/me")).ok()).toBe(true);
});


test("settings with revoked session redirects without a React hook crash", async ({ page }) => {
  await login(page, admin.email, admin.password);
  await page.goto("/w/acme/settings");
  await expect(page.getByLabel("워크스페이스 이름", { exact: true })).toBeVisible();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // Real logout revokes the session; retain its cookie to exercise server rejection.
  const cookies = await page.context().cookies();
  const result = await page.request.post("/api/v1/auth/logout");
  expect(result.ok()).toBe(true);
  await page.context().addCookies(cookies);
  await page.reload();
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("button", { name: "로그인", exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});
