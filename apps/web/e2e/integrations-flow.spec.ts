import { createHmac } from "node:crypto";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { expect, type Page, test } from "@playwright/test";
import { login } from "./helpers";

const owner = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "acme",
  workspaceName: "Acme 워크스페이스",
};

type Delivery = { headers: IncomingHttpHeaders; body: string };

// The e2e server runs with FVOCI_WEBHOOK_ALLOW_TARGETS=127.0.0.1 so this local receiver is allowed.
async function startReceiver(): Promise<{ server: Server; port: number; deliveries: Delivery[] }> {
  const deliveries: Delivery[] = [];
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      if (req.method === "POST") {
        deliveries.push({ headers: req.headers, body: Buffer.concat(chunks).toString("utf8") });
      }
      res.writeHead(200, { "content-type": "text/plain" });
      res.end("ok");
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return { server, port, deliveries };
}

function signatureMatches(delivery: Delivery, secret: string): boolean {
  const expected = `sha256=${createHmac("sha256", secret).update(delivery.body, "utf8").digest("hex")}`;
  return delivery.headers["x-fvoci-signature"] === expected;
}

function verbOf(delivery: Delivery): string | null {
  try {
    const parsed = JSON.parse(delivery.body) as { verb?: unknown };
    return typeof parsed.verb === "string" ? parsed.verb : null;
  } catch {
    return null;
  }
}

test("owner creates a signed webhook, receives project.created, then deletes it", async ({ page }) => {
  test.setTimeout(90_000);
  const receiver = await startReceiver();
  try {
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

    const hookUrl = `http://127.0.0.1:${receiver.port}/hook`;
    await page.goto(`/w/${owner.workspaceSlug}/settings`);

    // No GitHub App is configured under e2e: the section only reports the disconnected state.
    const github = page.locator("details").filter({
      has: page.locator("summary").filter({ hasText: /^GitHub$/ }),
    });
    await github.locator("summary").click();
    await expect(github.getByText("연결되지 않음")).toBeVisible();
    await expect(github.getByRole("button", { name: "GitHub App 설치" })).toBeVisible();

    const webhooks = page.locator("details").filter({
      has: page.locator("summary").filter({ hasText: /^웹훅$/ }),
    });
    await webhooks.locator("summary").click();
    await expect(webhooks.getByText("웹훅이 없습니다")).toBeVisible();
    await webhooks.getByRole("textbox", { name: "URL", exact: true }).fill(hookUrl);
    await webhooks.getByLabel("프로젝트 생성", { exact: true }).check();
    await webhooks.getByRole("button", { name: "추가", exact: true }).click();

    await expect(page.getByRole("status").filter({ hasText: "시크릿은 지금만 보입니다" })).toBeVisible();
    const revealed = page.getByRole("textbox", { name: "웹훅 서명 시크릿" });
    await expect(revealed).toBeVisible();
    const secret = await revealed.inputValue();
    expect(secret.length).toBeGreaterThan(16);
    await expect(page.getByText(hookUrl, { exact: true })).toBeVisible();

    await page.reload();
    await webhooks.locator("summary").click();
    await expect(page.getByText(hookUrl, { exact: true })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "웹훅 서명 시크릿" })).toHaveCount(0);
    await expect(page.getByText("시크릿은 지금만 보입니다")).toHaveCount(0);

    const workspacesRes = await page.request.get("/api/v1/me/workspaces");
    expect(workspacesRes.ok()).toBe(true);
    const workspace = (await workspacesRes.json()).items.find(
      (item: { slug: string }) => item.slug === owner.workspaceSlug,
    );
    expect(workspace).toBeTruthy();
    const projectRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
      data: { key: "HOOK", name: "웹훅 프로젝트", visibility: "workspace" },
    });
    expect(projectRes.status(), await projectRes.text()).toBe(201);

    await expect
      .poll(
        () =>
          receiver.deliveries.some(
            (delivery) => verbOf(delivery) === "project.created" && signatureMatches(delivery, secret),
          ),
        { timeout: 20_000 },
      )
      .toBe(true);

    await page.getByRole("button", { name: `삭제 ${hookUrl}` }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog.getByRole("heading", { name: "웹훅을 삭제할까요?" })).toBeVisible();
    await dialog.getByRole("button", { name: "삭제", exact: true }).click();
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await expect(page.getByText("웹훅이 없습니다")).toBeVisible();
    await expect(page.getByText(hookUrl, { exact: true })).toHaveCount(0);

    const listRes = await page.request.get(`/api/v1/workspaces/${workspace.id}/webhooks`);
    expect(listRes.ok()).toBe(true);
    expect((await listRes.json()).items).toEqual([]);
  } finally {
    receiver.server.closeAllConnections();
    await new Promise<void>((resolve) => receiver.server.close(() => resolve()));
  }
});

const INSTANCE_PATH = "/api/v1/instance";

function isInstanceRead(url: string): boolean {
  return new URL(url).pathname === INSTANCE_PATH;
}

/** The webhook test above runs setup first; alone (or after its failure) this still signs in. */
async function signInOwner(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(owner.familyName);
    await page.getByLabel("이름", { exact: true }).fill(owner.givenName);
    await page.getByLabel("이메일").fill(owner.email);
    await page.getByLabel("비밀번호").fill(owner.password);
    await page.getByLabel("워크스페이스 이름").fill(owner.workspaceName);
    await page.getByLabel("주소(영문)").fill(owner.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else if ((await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0) {
    await login(page, owner.email, owner.password);
  }
}

test("public features.ai gates the document AI menu; the server keeps its own AI gate", async ({
  page,
  request,
}) => {
  test.setTimeout(90_000);
  await signInOwner(page);

  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspace = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === owner.workspaceSlug,
  ) as { id: string };
  const docRes = await page.request.post(`/api/v1/workspaces/${workspace.id}/documents`, {
    data: { parentId: null, title: "AI 게이트 문서" },
  });
  expect(docRes.status(), await docRes.text()).toBe(201);
  const doc = (await docRes.json()) as { id: string; number: number };
  const docPath = `/w/${owner.workspaceSlug}/WIKI-${doc.number}`;
  const aiMenu = page.getByRole("group", { name: "AI 도구" });

  // Default instance: the public view says off and the menu is absent once that answer is in.
  const initial = await request.get(INSTANCE_PATH);
  expect(initial.ok()).toBe(true);
  expect((await initial.json()).values.features.ai).toBe(false);
  const instanceRead = page.waitForResponse((res) => isInstanceRead(res.url()));
  await page.goto(docPath);
  await instanceRead;
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(aiMenu).toHaveCount(0);

  // Turn it on from the real admin settings card without reloading the app, so the
  // document screen must pick the change up from the shared ["instance"] query.
  // WHY: the document shell has no admin link; a same-document history push keeps the SPA cache.
  await page.evaluate((path) => {
    window.history.pushState(null, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, "/settings/admin");
  const features = page.getByRole("region", { name: "기능 토글", exact: true });
  await expect(features.getByLabel("features.ai")).toBeEnabled();
  await expect(features.getByLabel("features.ai")).not.toBeChecked();
  await features.getByLabel("features.ai").check();
  const saved = page.waitForResponse(
    (res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await features.getByRole("button", { name: "저장", exact: true }).click();
  expect((await saved).status()).toBe(200);

  await page.goBack();
  await expect(page).toHaveURL(new RegExp(`${docPath}$`));
  await expect(aiMenu).toBeVisible();
  await page.reload();
  await expect(aiMenu).toBeVisible();

  // The flag is public and changes only the UI; anonymous callers still cannot write settings.
  const publicView = await request.get(INSTANCE_PATH);
  expect((await publicView.json()).values.features.ai).toBe(true);
  const anonPatch = await request.patch("/api/v1/admin/instance-settings", {
    data: { features: { ai: false } },
  });
  expect([401, 403]).toContain(anonPatch.status());

  // The server's own AI gate (FVOCI_AI_ENABLED/FVOCI_AI_SECRET) is unset in e2e: the menu shows
  // but the action answers 503 ai_unavailable and the buttons lock.
  const direct = await page.request.post(`/api/v1/workspaces/${workspace.id}/ai/summarize`, {
    data: { documentId: doc.id },
  });
  expect(direct.status()).toBe(503);
  expect((await direct.json()).code).toBe("ai_unavailable");
  await aiMenu.getByRole("button", { name: "요약", exact: true }).click();
  await expect(page.getByRole("alert").filter({ hasText: "AI 기능을 지금 사용할 수 없습니다" })).toBeVisible();
  await expect(aiMenu.getByRole("button", { name: "요약", exact: true })).toBeDisabled();

  // Back to the default: the card's reset writes null and the menu disappears again.
  await page.goto("/settings/admin");
  const reset = page.getByRole("region", { name: "기능 토글", exact: true });
  const cleared = page.waitForResponse(
    (res) => res.url().endsWith("/api/v1/admin/instance-settings") && res.request().method() === "PATCH",
  );
  await reset.getByRole("button", { name: "기본값으로", exact: true }).click();
  expect((await cleared).status()).toBe(200);
  const offRead = page.waitForResponse((res) => isInstanceRead(res.url()));
  await page.goto(docPath);
  await offRead;
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(aiMenu).toHaveCount(0);
});
