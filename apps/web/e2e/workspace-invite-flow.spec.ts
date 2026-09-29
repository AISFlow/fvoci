import { execFileSync } from "node:child_process";
import { expect, test } from "@playwright/test";
import { logout, watchCspViolations } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

const invited = {
  email: "second-human@example.com",
  password: "invitepass1",
  familyName: "박",
  givenName: "초대수락",
};

test("owner invites a second user who signs up, accepts, and appears in members", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const csp = watchCspViolations(page);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const guardedRequests: string[] = [];
  page.on("request", (request) => {
    if (/\/api\/v1\/(invitations\/|auth\/providers)/.test(request.url())) {
      guardedRequests.push(request.url());
    }
  });
  // Direct entry before installation follows the setup guard, without
  // starting public invitation/provider requests against an unready server.
  await page.goto("/invite/not-installed");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await expect(page.getByRole("button", { name: "시작하기" })).toBeVisible();
  expect(guardedRequests).toEqual([]);

  await page.getByLabel("성").fill(owner.familyName);
  await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
  await page.getByLabel("이메일").fill(owner.email);
  await page.getByLabel("비밀번호").fill(owner.password);
  await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
  await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);

  await page.goto("/w/acme/settings");
  await page.locator("summary").filter({ hasText: /^멤버$/ }).click();
  await page.getByLabel("초대할 이메일").fill(invited.email);
  await page.getByRole("button", { name: "초대", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "초대를 만들었습니다" })).toBeVisible();
  const inviteLink = page.getByRole("link").filter({ hasText: "/invite/" });
  await expect(inviteLink).toBeVisible();
  const href = await inviteLink.getAttribute("href");
  const token = href?.split("/invite/")[1];
  expect(token).toBeTruthy();

  // Expire a second real invitation in this run's isolated DB. The normal
  // invitation API creates it; the browser still reads the Rust endpoint.
  await page.getByLabel("초대할 이메일").fill("expired-invite@example.com");
  await page.getByRole("button", { name: "초대", exact: true }).click();
  await expect(inviteLink).not.toHaveAttribute("href", href!);
  const expiredHref = await inviteLink.getAttribute("href");
  expect(expiredHref).toBeTruthy();
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  if (!adminUrl || !container) throw new Error("isolated PostgreSQL fixture is required");
  const database = new URL(adminUrl).pathname.slice(1);
  const updated = execFileSync("docker", [
    "exec", container, "psql", "-U", "postgres", "-d", database,
    "-v", "ON_ERROR_STOP=1", "-c",
    "UPDATE fvoci.invitations SET expires_at = now() - interval '1 second' WHERE email = 'expired-invite@example.com' AND accepted_at IS NULL",
  ], { encoding: "utf8", stdio: "pipe" });
  expect(updated.trim()).toBe("UPDATE 1");

  await logout(page);

  for (const path of ["/invite/not-a-valid-token", expiredHref!]) {
    await page.goto(path);
    await expect(page.getByRole("alert")).toContainText("초대를 찾을 수 없거나 만료되었습니다");
    await expect(page.getByRole("button", { name: "수락", exact: true })).toHaveCount(0);
    await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  }

  await page.goto(`/invite/${token}`);
  await expect(page.getByRole("heading", { name: /초대 수락/ })).toBeVisible();
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  await page.reload();
  await expect(page.getByRole("heading", { name: /초대 수락/ })).toBeVisible();
  await expect(page.locator("#root")).toHaveAttribute("data-v-app", "");
  await page.getByLabel("이메일").fill("wrong-invitee@example.com");
  await page.getByLabel("성").fill(invited.familyName);
  await page.getByLabel("이름", { exact: true }).fill(invited.givenName);
  await page.getByLabel("비밀번호").fill(invited.password);
  const rejected = page.waitForResponse((response) =>
    response.url().endsWith(`/api/v1/invitations/${token}/accept`) &&
    response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "수락" }).click();
  const rejection = await rejected;
  expect(rejection.status()).toBe(401);
  expect((await rejection.json()).code).toBe("cannot_accept_invitation");
  await expect(page.getByRole("alert")).toBeVisible();
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  // A refused email leaves the invitation usable for its intended account.
  await page.getByLabel("이메일").fill(invited.email);
  await page.getByRole("button", { name: "수락" }).click();

  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByText(owner.workspaceName)).toBeVisible();

  await page.goto("/w/acme/settings");
  await page.locator("summary").filter({ hasText: /^멤버$/ }).click();
  await expect(page.getByText(invited.email)).toBeVisible();
  await expect(page.getByText("박초대수락")).toBeVisible();
  await expect(page.getByText("멤버", { exact: true }).nth(1)).toBeVisible();
  // The accepted token cannot grant membership a second time.
  await page.goto(`/invite/${token}`);
  await expect(page.getByRole("alert")).toContainText("초대를 찾을 수 없거나 만료되었습니다");
  await expect(page.getByRole("button", { name: "수락", exact: true })).toHaveCount(0);
  expect(errors).toEqual([]);
  expect(csp).toEqual([]);
});
