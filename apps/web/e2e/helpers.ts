import { execFileSync } from "node:child_process";
import path from "node:path";
import { expect, type Page } from "@playwright/test";

export function createE2eUser(
  email: string,
  password: string,
  givenName: string,
  options?: {
    familyName?: string;
    workspaceSlug?: string;
    membershipRole?: string;
  },
): void {
  const root = path.resolve(import.meta.dirname, "../../..");
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  if (!adminUrl) {
    throw new Error("FVOCI_E2E_ADMIN_DATABASE_URL is required for DB fixtures");
  }
  execFileSync(
    path.join(process.env.CARGO_TARGET_DIR ?? path.join(root, "target"), "debug/fvoci-e2e-fixture"),
    [],
    {
      env: {
        ...process.env,
        DATABASE_URL: adminUrl,
        PASSWORD_PEPPER_KEYS:
          process.env.PASSWORD_PEPPER_KEYS ??
          '{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}',
        PASSWORD_PEPPER_ACTIVE_KEY_ID: process.env.PASSWORD_PEPPER_ACTIVE_KEY_ID ?? "test",
        E2E_USER_EMAIL: email,
        E2E_USER_PASSWORD: password,
        E2E_USER_GIVEN_NAME: givenName,
        ...(options?.familyName ? { E2E_USER_FAMILY_NAME: options.familyName } : {}),
        ...(options?.workspaceSlug ? { E2E_WORKSPACE_SLUG: options.workspaceSlug } : {}),
        ...(options?.membershipRole ? { E2E_MEMBERSHIP_ROLE: options.membershipRole } : {}),
      },
      stdio: "pipe",
    },
  );
}

// Logout ends on /login once the session is gone; navigating earlier races the
// in-flight logout and /login redirects the still-authenticated page away.
export async function logout(page: Page): Promise<void> {
  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);
}

export async function login(page: Page, email: string, password: string): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("이메일").fill(email);
  await page.getByLabel("비밀번호").fill(password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await page.waitForURL(/\/$/);
}

export async function createTasksViaApi(
  page: Page,
  workspaceId: string,
  projectId: string,
  titles: readonly string[],
  statusId: string,
): Promise<void> {
  const chunkSize = 10;
  for (let offset = 0; offset < titles.length; offset += chunkSize) {
    const chunk = titles.slice(offset, offset + chunkSize);
    const responses = await Promise.all(
      chunk.map((title) =>
        page.request.post(`/api/v1/workspaces/${workspaceId}/projects/${projectId}/tasks`, {
          data: { title, type: "task", statusId },
        }),
      ),
    );
    for (const [index, response] of responses.entries()) {
      if (response.status() !== 201) {
        throw new Error(
          `create task failed: title=${chunk[index]} status=${response.status()} body=${await response.text()}`,
        );
      }
    }
  }
}
