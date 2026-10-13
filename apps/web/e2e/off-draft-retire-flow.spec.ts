/** Retired dirty OFF drafts (maintainer decision 2026-10-12): kept in the tab,
 * offered back to the same actor and credential, discarded only on request,
 * never sent once access is gone (copy/download only) and invisible to any
 * other account. The normal group supplies the isolated PostgreSQL app role
 * and native binaries; this spec owns one realtime-OFF server on port 0. */
import { randomBytes, randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { appendFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import {
  expect,
  test as base,
  type Browser,
  type BrowserContext,
  type Cookie,
  type Page,
  type Request,
} from "@playwright/test";
import { createE2eUser, flowSchemas, login, readJson } from "./helpers";
import {
  ownedServerChildEnv,
  processGroupMembers,
  readProcMember,
  signalOwnedGroup,
} from "../e2e-pending/collab-restart";
import { createRustReadinessReader } from "../e2e-pending/tb-a-readiness";

const STORAGE_PREFIX = "fvoci:off-wiki:1:";
const admin = { email: "Admin@Example.COM", password: "supersecret1" };
const suffix = randomBytes(4).toString("hex");
const member = { email: `draft-member-${suffix}@example.com`, password: "memberpass1" };
const other = { email: `draft-other-${suffix}@example.com`, password: "otherpass1" };

type OffApp = { baseUrl: string };
const test = base.extend<object, { offApp: OffApp }>({
  offApp: [
    // Playwright requires the object pattern; this worker fixture has no dependencies.
    // eslint-disable-next-line no-empty-pattern
    async ({}, use, workerInfo) => {
      const binary = process.env.FVOCI_E2E_SERVER_BIN;
      const engine = process.env.FVOCI_COLLAB_ENGINE;
      if (!binary || !existsSync(binary)) throw new Error("FVOCI_E2E_SERVER_BIN missing");
      if (!engine || !existsSync(engine)) throw new Error("OFF mode requires FVOCI_COLLAB_ENGINE");
      if (!process.env.DATABASE_APP_URL) throw new Error("isolated app DB URL missing");
      const runDir = path.join(
        workerInfo.project.outputDir,
        `off-draft-retire-${String(workerInfo.workerIndex)}`,
      );
      const storage = path.join(runDir, "storage");
      mkdirSync(storage, { recursive: true, mode: 0o700 });
      const logPath = path.join(runDir, "server.log");
      const child: ChildProcess = spawn(binary, [], {
        detached: true,
        env: {
          ...ownedServerChildEnv("127.0.0.1:0"),
          FVOCI_REALTIME_MODE: "off",
          FVOCI_STORAGE_DIR: storage,
          RUST_LOG: "warn,tower_http=debug",
        },
        stdio: ["ignore", "pipe", "pipe"],
      });
      const pid = child.pid;
      if (!pid) throw new Error("OFF server spawn returned no PID");
      const identity = readProcMember(pid);
      const stop = async () => {
        const owners = processGroupMembers(pid);
        if (child.exitCode === null && child.signalCode === null) {
          const exited = once(child, "exit");
          if (readProcMember(pid)?.starttime !== identity?.starttime)
            throw new Error("OFF server ownership changed before cleanup");
          // Let the server reap its native helpers before checking the group.
          process.kill(pid, "SIGTERM");
          await exited;
        }
        const deadline = Date.now() + 5_000;
        while (processGroupMembers(pid).length && Date.now() < deadline) await delay(50);
        if (processGroupMembers(pid).length) {
          signalOwnedGroup(pid, owners);
          throw new Error("OFF server shutdown left process group members");
        }
      };
      try {
        const baseUrl = await new Promise<string>((resolve, reject) => {
          const timer = setTimeout(() => {
            reject(new Error("OFF server startup deadline"));
          }, 30_000);
          const readiness = createRustReadinessReader((url) => {
            clearTimeout(timer);
            resolve(url);
          });
          for (const stream of ["stdout", "stderr"] as const)
            child[stream]?.on("data", (bytes: Buffer) => {
              appendFileSync(logPath, bytes);
              readiness(stream, bytes);
            });
          child.once("error", (error) => {
            clearTimeout(timer);
            reject(error);
          });
          child.once("exit", (code) => {
            clearTimeout(timer);
            reject(new Error(`OFF server exited before readiness: ${String(code)}`));
          });
        });
        await use({ baseUrl });
      } finally {
        await stop();
      }
    },
    { scope: "worker" },
  ],
});

type Target = { id: string; url: string; body: string };
let workspaceId = "";
let workspaceSlug = "";
let memberId = "";
const cookies = new Map<string, Cookie[]>();

async function signIn(page: Page, who: { email: string; password: string }): Promise<void> {
  await login(page, who.email, who.password);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
}

async function actor(
  browser: Browser,
  baseURL: string,
  who: { email: string },
): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({
    baseURL,
    permissions: ["clipboard-read", "clipboard-write"],
  });
  const saved = cookies.get(who.email);
  if (saved) await context.addCookies(saved);
  return { context, page: await context.newPage() };
}

async function document(page: Page, title: string): Promise<Target> {
  const response = await page.request.post(`/api/v1/workspaces/${workspaceId}/documents`, {
    data: { commandId: randomUUID(), parentId: null, title },
  });
  expect(response.status()).toBe(201);
  const meta = await readJson(response, flowSchemas.document);
  if (!meta.displayId) throw new Error("wiki display ID missing");
  return {
    id: meta.id,
    url: `/w/${workspaceSlug}/${meta.displayId}`,
    body: `/api/v1/workspaces/${workspaceId}/documents/${meta.id}/body/versioned`,
  };
}

const editor = (page: Page) => page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]');

async function openOff(page: Page, target: Target): Promise<void> {
  const response = await page.goto(target.url);
  expect(response?.status()).toBe(200);
  await expect(page.locator('[data-body-mode="off"]')).toBeVisible();
  await expect(editor(page)).toBeVisible();
  await expect(page.locator("[data-collab-status]")).toHaveCount(0);
}

async function typeDraft(page: Page, text: string): Promise<void> {
  await editor(page).click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type(text);
  await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
    "data-body-persisted",
    "false",
  );
  await expect(editor(page)).toHaveText(text);
}

async function saveBody(page: Page, target: Target): Promise<void> {
  const saved = page.waitForResponse(
    (response) =>
      response.request().method() === "PUT" && new URL(response.url()).pathname === target.body,
  );
  await page.getByRole("button", { name: "저장", exact: true }).click();
  expect((await saved).status()).toBe(200);
  await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
    "data-body-persisted",
    "true",
  );
}

async function leave(page: Page): Promise<void> {
  // Same tab: sessionStorage is the tab-owned draft area.
  await page.goto(`/w/${workspaceSlug}/wiki`);
  await expect(page.locator('[data-body-mode="off"]')).toHaveCount(0);
}

async function bodyText(page: Page, target: Target): Promise<string> {
  const response = await page.request.get(target.body);
  expect(response.status()).toBe(200);
  const body = (await response.json()) as { contentJson: unknown; targetId: string };
  expect(body.targetId).toBe(target.id);
  return JSON.stringify(body.contentJson);
}

async function slots(page: Page): Promise<string[]> {
  return page.evaluate(
    (prefix) => Object.keys(sessionStorage).filter((key) => key.startsWith(prefix)),
    STORAGE_PREFIX,
  );
}

function sends(page: Page): Request[] {
  const sent: Request[] = [];
  page.on("request", (request) => {
    if (request.method() !== "GET" && new URL(request.url()).pathname.startsWith("/api/"))
      sent.push(request);
  });
  return sent;
}

test.beforeAll(async ({ browser, offApp }) => {
  const context = await browser.newContext({ baseURL: offApp.baseUrl });
  try {
    const page = await context.newPage();
    const setup = (await (await page.request.get("/api/v1/setup")).json()) as { needed: boolean };
    if (setup.needed) {
      await page.goto("/setup");
      await page.getByLabel("성").fill("보존");
      await page.getByLabel("이름", { exact: true }).fill("초안");
      await page.getByLabel("이메일").fill(admin.email);
      await page.getByLabel("비밀번호").fill(admin.password);
      await page.getByLabel("워크스페이스 이름").fill("Draft retire");
      await page.getByLabel("주소(영문)").fill(`draft-${suffix}`);
      await page.getByRole("button", { name: "시작하기" }).click();
      await expect(page).toHaveURL(/\/$/);
    } else await signIn(page, admin);
    workspaceSlug = `draft-${suffix}`;
    let workspaces = await readJson(
      await page.request.get("/api/v1/me/workspaces"),
      flowSchemas.workspaces,
    );
    if (!workspaces.items.some((item) => item.slug === workspaceSlug)) {
      const created = await page.request.post("/api/v1/workspaces", {
        data: { name: "Draft retire", slug: workspaceSlug },
      });
      expect(created.status()).toBe(201);
      workspaces = await readJson(
        await page.request.get("/api/v1/me/workspaces"),
        flowSchemas.workspaces,
      );
    }
    const workspace = workspaces.items.find((item) => item.slug === workspaceSlug);
    if (!workspace) throw new Error("draft workspace missing");
    workspaceId = workspace.id;
    const setupStatus = (await (await page.request.get("/api/v1/setup")).json()) as {
      realtimeMode?: string;
    };
    expect(setupStatus.realtimeMode).toBe("off");
    cookies.set(admin.email, await context.cookies());
    for (const [who, name] of [
      [member, "멤버"],
      [other, "다른"],
    ] as const) {
      createE2eUser(who.email, who.password, name, {
        familyName: "초안",
        workspaceSlug,
        membershipRole: "member",
      });
      await context.clearCookies();
      await signIn(page, who);
      cookies.set(who.email, await context.cookies());
      if (who === member) {
        const me = (await (await page.request.get("/api/v1/auth/me")).json()) as {
          userId: string;
        };
        memberId = me.userId;
      }
    }
  } finally {
    await context.close();
  }
});

test("(a) a dirty draft left behind is offered back, then its save reads back in a new client", async ({
  browser,
  offApp,
}) => {
  const owner = await actor(browser, offApp.baseUrl, admin);
  const fresh = await actor(browser, offApp.baseUrl, admin);
  try {
    const target = await document(owner.page, "복구 제안");
    await openOff(owner.page, target);
    const draft = `left behind 초안 ${suffix}`;
    await typeDraft(owner.page, draft);
    await leave(owner.page);
    await openOff(owner.page, target);
    const notice = owner.page.getByTestId("off-draft-restored");
    await expect(notice).toBeVisible();
    await expect(notice.getByRole("button", { name: "초안 버리기", exact: true })).toBeVisible();
    await expect(editor(owner.page)).toHaveText(draft);
    expect(await bodyText(owner.page, target)).not.toContain(draft);
    await saveBody(owner.page, target);
    await expect(notice).toHaveCount(0);
    expect(await bodyText(fresh.page, target)).toContain(draft);
    await openOff(fresh.page, target);
    await expect(editor(fresh.page)).toHaveText(draft);
    await expect(fresh.page.getByTestId("off-draft-restored")).toHaveCount(0);
  } finally {
    await Promise.all([owner.context.close(), fresh.context.close()]);
  }
});

test("(c) an explicit discard removes the tab slot and nothing is offered again", async ({
  browser,
  offApp,
}) => {
  const owner = await actor(browser, offApp.baseUrl, admin);
  try {
    const sent = sends(owner.page);
    const target = await document(owner.page, "버리기");
    await openOff(owner.page, target);
    // The editor's own mount normalization of an empty body is not offered.
    await leave(owner.page);
    await openOff(owner.page, target);
    await expect(owner.page.getByTestId("off-draft-restored")).toHaveCount(0);
    const base = `saved base ${suffix}`;
    await typeDraft(owner.page, base);
    await saveBody(owner.page, target);
    const draft = `discard me ${suffix}`;
    await typeDraft(owner.page, draft);
    await leave(owner.page);
    expect((await slots(owner.page)).some((key) => key.includes(target.id))).toBe(true);
    await openOff(owner.page, target);
    const notice = owner.page.getByTestId("off-draft-restored");
    await expect(notice).toBeVisible();
    await expect(editor(owner.page)).toHaveText(draft);
    await notice.getByRole("button", { name: "초안 버리기", exact: true }).click();
    await expect(notice).toHaveCount(0);
    await expect(editor(owner.page)).toHaveText(base);
    await expect(owner.page.locator('[data-body-mode="off"]')).toHaveAttribute(
      "data-body-persisted",
      "true",
    );
    expect((await slots(owner.page)).some((key) => key.includes(target.id))).toBe(false);
    await leave(owner.page);
    await openOff(owner.page, target);
    await expect(owner.page.getByTestId("off-draft-restored")).toHaveCount(0);
    await expect(editor(owner.page)).toHaveText(base);
    expect(await bodyText(owner.page, target)).not.toContain(draft);
    // The one send is the deliberate base save; the discarded draft never left.
    expect(sent.filter((request) => request.url().includes(target.id))).toHaveLength(1);
    expect(sent.filter((request) => request.postData()?.includes(draft))).toEqual([]);
  } finally {
    await owner.context.close();
  }
});

test("(d) after logout another account in the same tab never sees the draft", async ({
  browser,
  offApp,
}) => {
  const tab = await actor(browser, offApp.baseUrl, { email: "fresh login" });
  try {
    const sent = sends(tab.page);
    await signIn(tab.page, member);
    const target = await document(tab.page, "로그아웃");
    await openOff(tab.page, target);
    const draft = `member private ${suffix}`;
    await typeDraft(tab.page, draft);
    await tab.page.getByRole("button", { name: "로그아웃", exact: true }).click();
    await expect(tab.page).toHaveURL(/\/login$/);
    // The retired draft stays in this tab under the member's own scope.
    expect((await slots(tab.page)).some((key) => key.includes(target.id))).toBe(true);
    await signIn(tab.page, other);
    await openOff(tab.page, target);
    await expect(editor(tab.page)).not.toContainText(draft);
    await expect(tab.page.getByTestId("off-draft-restored")).toHaveCount(0);
    await tab.page.goto("/?denied=workspace");
    await expect(tab.page.getByRole("alert").first()).toBeVisible();
    await expect(tab.page.getByTestId("off-draft-retained")).toHaveCount(0);
    await expect(tab.page.getByText(draft)).toHaveCount(0);
    expect(sent.filter((request) => request.postData()?.includes(draft))).toEqual([]);
  } finally {
    await tab.context.close();
  }
});

test("(b) membership revoke: zero sends, reopen shows the denial with export only", async ({
  browser,
  offApp,
}) => {
  const owner = await actor(browser, offApp.baseUrl, admin);
  const peer = await actor(browser, offApp.baseUrl, member);
  try {
    const target = await document(owner.page, "권한 철회");
    const sent = sends(peer.page);
    await openOff(peer.page, target);
    const draft = `revoked private 초안 ${suffix}`;
    await typeDraft(peer.page, draft);
    const revokedAt = sent.length;
    const revoke = await owner.page.request.delete(
      `/api/v1/workspaces/${workspaceId}/members/${memberId}`,
    );
    expect(revoke.status()).toBe(200);
    // The actual access stream evicts the page; that is the client's signal.
    await expect(peer.page).toHaveURL(/\/\?denied=workspace$/);
    const retained = peer.page.getByTestId("off-draft-retained");
    await expect(retained).toHaveCount(1);
    await expect(retained).toContainText(draft.slice(0, 20));
    await peer.page.goto(target.url);
    await expect(peer.page).toHaveURL(/\/\?denied=workspace$/);
    await expect(peer.page.getByRole("alert").first()).toBeVisible();
    await expect(retained).toHaveCount(1);
    await expect(retained.getByRole("button")).toHaveText(["초안 복사", "초안 내려받기"]);
    await retained.getByRole("button", { name: "초안 복사", exact: true }).click();
    expect(await peer.page.evaluate(() => navigator.clipboard.readText())).toContain(draft);
    const [download] = await Promise.all([
      peer.page.waitForEvent("download"),
      retained.getByRole("button", { name: "초안 내려받기", exact: true }).click(),
    ]);
    expect(download.suggestedFilename()).toMatch(/\.md$/);
    expect(readFileSync(await download.path(), "utf8")).toContain(draft);
    expect(sent.slice(revokedAt)).toEqual([]);
    expect(sent.filter((request) => request.postData()?.includes(draft))).toEqual([]);
    expect(await bodyText(owner.page, target)).not.toContain(draft);
    // The same tab after logout: another account never lists it.
    await peer.page.getByRole("button", { name: "로그아웃", exact: true }).click();
    await expect(peer.page).toHaveURL(/\/login$/);
    await signIn(peer.page, other);
    await peer.page.goto("/?denied=workspace");
    await expect(peer.page.getByRole("alert").first()).toBeVisible();
    await expect(peer.page.getByTestId("off-draft-retained")).toHaveCount(0);
    await expect(peer.page.getByText(draft)).toHaveCount(0);
  } finally {
    await Promise.all([owner.context.close(), peer.context.close()]);
  }
});
