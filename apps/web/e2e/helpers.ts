import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, type Page } from "@playwright/test";
import { z } from "zod";
export type CapturedMail = {
  from: string;
  to: string;
  data: string;
  text: string;
  ts: number;
};
export function capturedMails(): CapturedMail[] {
  const capture = process.env.FVOCI_E2E_SMTP_CAPTURE;
  if (!capture) {
    throw new Error("FVOCI_E2E_SMTP_CAPTURE is required");
  }
  try {
    const raw = readFileSync(capture, "utf8");
    return raw
      .split("\n")
      .filter((line) => line.trim().length > 0)
      .map((line) => JSON.parse(line) as CapturedMail);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      return [];
    }
    throw err;
  }
}

export async function waitForCapturedMail(
  predicate: (mail: CapturedMail) => boolean,
): Promise<CapturedMail> {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const match = capturedMails().find(predicate);
    if (match) {
      return match;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("captured mail did not arrive");
}

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

// Collects Content-Security-Policy violations the browser reports for a page
// (Chromium logs every blocked script/style/connect/frame as a console error).
// Assert the list is empty at the end of a flow to prove the global policy does
// not block the app.
export function watchCspViolations(page: Page): string[] {
  const violations: string[] = [];
  page.on("console", (message) => {
    const text = message.text();
    if (
      /Content Security Policy|Refused to (load|execute|apply|connect|frame|create)/i.test(text)
    ) {
      violations.push(text);
    }
  });
  return violations;
}

// Logout ends on /login once the session is gone; navigating earlier races the
// in-flight logout and /login redirects the still-authenticated page away.
export async function logout(page: Page): Promise<void> {
  await page.getByRole("button", { name: "로그아웃" }).click();
  await expect(page).toHaveURL(/\/login$/);
  // React routing changes the URL before NoRoute loads the Vue login app.
  // Wait for its form before another navigation or evaluation uses the page.
  await expect(page.getByLabel("이메일")).toBeVisible();
  await expect(page.getByLabel("비밀번호")).toBeVisible();
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
        const fixtureValue1 = chunk[index];
        if (fixtureValue1 === undefined) throw new Error("Missing fixture value: chunk[index]");
        throw new Error(
          `create task failed: title=${fixtureValue1} status=${String(response.status())} body=${await response.text()}`,
        );
      }
    }
  }
}
// Validate the fields a flow consumes while retaining the full response for its
// original equality and secret-retention assertions.
export async function readJson<T>(
  response: {
    json(): Promise<unknown>;
  },
  schema: import("zod").ZodType<T>,
): Promise<T> {
  return schema.parse(await response.json());
}
const idSchema = z.object({ id: z.string() }).passthrough();
const numberedSchema = idSchema.extend({ number: z.number() });
const itemSchema = numberedSchema.extend({ title: z.string() });
const workspaceSchema = idSchema.extend({ slug: z.string(), name: z.string() });
const projectSchema = idSchema.extend({
  key: z.string(),
  rootDocumentId: z.string().nullable(),
  name: z.string(),
});
const documentSchema = itemSchema.extend({
  displayId: z.string().optional(),
  parentId: z.string().nullable(),
  icon: z.string().nullable(),
  status: z.string(),
});
const tagSchema = idSchema.extend({ name: z.string() });
export const flowSchemas = {
  unknown: z.unknown(),
  items: z.object({ items: z.array(z.unknown()) }).passthrough(),
  push: z
    .object({
      endpoint: z.string(),
      keys: z.object({ p256dh: z.string(), auth: z.string() }).passthrough(),
    })
    .passthrough(),
  password: z.object({ currentPassword: z.string() }).passthrough(),
  pending: z.object({ pending: z.array(z.unknown()) }).passthrough(),
  mfa: z.object({ mfaToken: z.string() }).passthrough(),
  stars: z
    .object({ items: z.array(z.object({ targetId: z.string() }).passthrough()) })
    .passthrough(),
  trash: z
    .object({
      items: z.array(
        z.object({ id: z.string(), kind: z.string(), title: z.string() }).passthrough(),
      ),
    })
    .passthrough(),
  consents: z
    .object({
      members: z.array(
        z
          .object({
            userId: z.string(),
            consents: z.array(z.object({ kind: z.string(), version: z.number() }).passthrough()),
          })
          .passthrough(),
      ),
    })
    .passthrough(),
  id: idSchema,
  numbered: numberedSchema,
  item: itemSchema,
  workspace: workspaceSchema,
  workspaces: z
    .object({ items: z.array(workspaceSchema.extend({ documentCount: z.number() })) })
    .passthrough(),
  project: projectSchema,
  projects: z.object({ items: z.array(projectSchema) }).passthrough(),
  document: documentSchema,
  createdDocument: documentSchema.extend({ displayId: z.string() }),
  documents: z.object({ items: z.array(documentSchema) }).passthrough(),
  tasks: z.object({ items: z.array(itemSchema) }).passthrough(),
  search: z
    .object({ items: z.array(idSchema.extend({ title: z.string(), type: z.string() })) })
    .passthrough(),
  tag: tagSchema,
  tags: z.object({ items: z.array(tagSchema) }).passthrough(),
  members: z
    .object({ items: z.array(z.object({ userId: z.string(), email: z.string() }).passthrough()) })
    .passthrough(),
  groups: z.object({ items: z.array(tagSchema) }).passthrough(),
  setup: z.object({ needed: z.boolean() }).passthrough(),
  user: z.object({ userId: z.string(), givenName: z.string(), email: z.string() }).passthrough(),
  error: z.object({ code: z.string() }).passthrough(),
  body: z.object({ contentJson: z.unknown() }).passthrough(),
  share: idSchema.extend({ url: z.string() }),
  upload: z
    .object({
      attachmentId: z.string(),
      partSizeBytes: z.number(),
      parts: z.array(z.object({ partNumber: z.number(), url: z.string() }).passthrough()),
    })
    .passthrough(),
  token: idSchema.extend({ token: z.string(), workspaceId: z.string() }),
  tokens: z.object({ items: z.array(idSchema) }).passthrough(),
  notifications: z
    .object({
      items: z.array(
        idSchema.extend({
          verb: z.string(),
          displayId: z.string().nullable(),
          readAt: z.string().nullable(),
          payload: z.object({ title: z.string().optional() }).passthrough().nullable(),
        }),
      ),
    })
    .passthrough(),
  count: z.object({ count: z.number() }).passthrough(),
  prefs: z.object({ inApp: z.boolean(), mailDigest: z.boolean() }).passthrough(),
  version: z.object({ version: z.number() }).passthrough(),
  instance: z
    .object({
      values: z
        .object({
          branding: z.object({ name: z.string() }).passthrough(),
          features: z.object({ ai: z.boolean() }).passthrough(),
          webPushPublicKey: z.string().nullable(),
          operator: z.record(z.string(), z.string().nullable()),
        })
        .passthrough(),
    })
    .passthrough(),
};
