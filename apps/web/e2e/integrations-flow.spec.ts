import { createHmac } from "node:crypto";
import { createServer, type IncomingHttpHeaders, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { expect, test } from "@playwright/test";

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
