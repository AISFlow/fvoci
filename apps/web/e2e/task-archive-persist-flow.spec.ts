import { expect, test, type Page, type Route } from "@playwright/test";
import { decodeHocuspocusFrame, frameBytes, persistParts } from "../e2e-pending/collab-wire";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "tap",
  workspaceName: "Task Archive Persist",
};

const persistFailedMessage =
  "본문 저장을 확인하지 못해 보관하지 않았습니다. 연결을 확인하고 다시 시도해 주세요.";

type CollabWireLog = {
  sent: ReturnType<typeof decodeHocuspocusFrame>[];
};

function attachCollabWire(page: Page): CollabWireLog {
  const log: CollabWireLog = { sent: [] };
  page.on("websocket", (ws) => {
    if (!ws.url().includes("/collab")) return;
    ws.on("framesent", (frame) => {
      const decoded = decodeHocuspocusFrame(frameBytes(frame.payload));
      if (decoded) log.sent.push(decoded);
    });
  });
  return log;
}

function sentPersistRequests(log: CollabWireLog): string[] {
  return log.sent.flatMap((frame) => {
    if (!frame || frame.kind !== "stateless") return [];
    const parts = persistParts(frame.payload);
    return parts?.kind === "request" ? [parts.id] : [];
  });
}

function toFrameBytes(message: string | Buffer): Uint8Array {
  if (typeof message === "string") return frameBytes(message);
  return new Uint8Array(message);
}

/** Hold server `persisted:` ack frames until release() (archive persist barrier). */
function installPersistAckHold(page: Page): { release: () => void } {
  const gates: Array<() => void> = [];
  let holdAcks = true;
  page.routeWebSocket(/\/collab/, (ws) => {
    const server = ws.connectToServer();
    ws.onMessage((message) => {
      server.send(message);
    });
    server.onMessage((message) => {
      const decoded = decodeHocuspocusFrame(toFrameBytes(message));
      if (decoded?.kind === "stateless") {
        const parts = persistParts(decoded.payload);
        if (holdAcks && parts?.kind === "done") {
          let releaseGate!: () => void;
          const gate = new Promise<void>((resolve) => {
            releaseGate = resolve;
          });
          gates.push(releaseGate);
          void gate.then(() => ws.send(message));
          return;
        }
      }
      ws.send(message);
    });
  });
  return {
    release: () => {
      holdAcks = false;
      for (const open of gates.splice(0)) open();
    },
  };
}

async function ensureSetup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그아웃" }))
      .or(page.getByRole("button", { name: "로그인", exact: true })),
  ).toBeVisible();
  if ((await page.getByRole("button", { name: "시작하기" }).count()) > 0) {
    await page.getByLabel("성").fill(admin.familyName);
    await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
    await page.getByLabel("이메일").fill(admin.email);
    await page.getByLabel("비밀번호").fill(admin.password);
    await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
    await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
    return;
  }
  if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
}

async function taskJson(page: Page, wsId: string, taskId: string) {
  const res = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${taskId}`);
  expect(res.ok()).toBe(true);
  return res.json() as Promise<{ contentJson: unknown; archivedAt: string | null }>;
}

async function openEditableTask(
  page: Page,
  wire: CollabWireLog,
  projectKey: string,
): Promise<{ wsId: string; task: { id: string; number: number }; bodyText: string }> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  const wsId = (await workspacesRes.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  ).id as string;

  const projectRes = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: projectKey, name: "Archive Persist", visibility: "workspace" },
  });
  expect(projectRes.status(), await projectRes.text()).toBe(201);
  const project = (await projectRes.json()) as { id: string };
  const taskRes = await page.request.post(
    `/api/v1/workspaces/${wsId}/projects/${project.id}/tasks`,
    { data: { title: "보관 전 본문" } },
  );
  expect(taskRes.status()).toBe(201);
  const task = (await taskRes.json()) as { id: string; number: number };

  await page.goto(`/w/${admin.workspaceSlug}/${projectKey}-${task.number}`);
  await expect(page.getByRole("heading", { name: "보관 전 본문" })).toBeVisible({
    timeout: 15_000,
  });
  const body = page.getByTestId("task-body");
  await expect(body).toBeVisible({ timeout: 15_000 });
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  const editor = body.locator(".fvoci-editor .ProseMirror");
  await editor.click();
  const bodyText = `한글 본문 🎯 보관 ${Date.now()}`;
  await page.keyboard.type(bodyText);
  wire.sent.length = 0;
  return { wsId, task, bodyText };
}

function holdArchivePatch(page: Page, wsId: string, taskId: string): { release: () => void } {
  let releaseHold!: () => void;
  let gateOpen = false;
  const held = new Promise<void>((resolve) => {
    releaseHold = () => {
      gateOpen = true;
      resolve();
    };
  });
  const matchUrl = `**/api/v1/workspaces/${wsId}/tasks/${taskId}`;
  const holdPatch = async (route: Route) => {
    const request = route.request();
    if (
      !gateOpen &&
      request.method() === "PATCH" &&
      (request.postDataJSON() as { archived?: boolean } | null)?.archived === true
    ) {
      await held;
    }
    await route.continue();
  };
  void page.route(matchUrl, holdPatch);
  return { release: releaseHold };
}

test("archive persists collaborative body then restores after unarchive", async ({ page }) => {
  const wire = attachCollabWire(page);
  await ensureSetup(page);
  const { wsId, task, bodyText } = await openEditableTask(page, wire, "ZT701");
  const body = page.getByTestId("task-body");
  const editor = body.locator(".fvoci-editor .ProseMirror");

  await page.getByRole("button", { name: "보관", exact: true }).click();
  await expect(page.getByText("보관된 태스크입니다")).toBeVisible({ timeout: 15_000 });
  await expect(editor).toHaveAttribute("contenteditable", "false");

  await expect
    .poll(async () => {
      const res = await page.request.get(`/api/v1/workspaces/${wsId}/tasks/${task.id}`);
      const json = await res.json();
      return JSON.stringify(json.contentJson);
    })
    .toContain("한글 본문");
  await expect
    .poll(async () => JSON.stringify((await taskJson(page, wsId, task.id)).contentJson))
    .toContain("bullseye");

  await page.reload();
  await expect(body).toBeVisible({ timeout: 15_000 });
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  await expect(editor).toContainText(bodyText);

  await page.getByRole("button", { name: "복원", exact: true }).click();
  await expect.poll(async () => (await taskJson(page, wsId, task.id)).archivedAt).toBeNull();
  await page.reload();
  await expect(body).toBeVisible({ timeout: 15_000 });
  await expect(body.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 30_000 });
  await expect(editor).toHaveAttribute("contenteditable", "true");
  await expect(editor).toContainText(bodyText);
});

test("archive holds editor read-only while persist and archive PATCH are in flight", async ({
  page,
}) => {
  const wire = attachCollabWire(page);
  await ensureSetup(page);
  const { wsId, task, bodyText } = await openEditableTask(page, wire, "ZT702");
  const patchHold = holdArchivePatch(page, wsId, task.id);
  const editor = page.getByTestId("task-body").locator(".fvoci-editor .ProseMirror");
  const archiveButton = page.getByRole("button", { name: "보관", exact: true });

  const archivePatch = page.waitForRequest(
    (request) =>
      request.method() === "PATCH" &&
      request.url().includes(`/tasks/${task.id}`) &&
      (request.postDataJSON() as { archived?: boolean } | null)?.archived === true,
  );
  const archivePatchDone = page.waitForResponse(
    (response) =>
      response.request().method() === "PATCH" &&
      response.url().includes(`/tasks/${task.id}`) &&
      response.ok(),
  );

  await archiveButton.click();
  await archivePatch;

  await expect(editor).toHaveAttribute("contenteditable", "false");
  await expect(archiveButton).toBeDisabled();

  const beforeRace = await editor.innerText();
  await page.keyboard.type("레이스 입력");
  expect(await editor.innerText()).toBe(beforeRace);

  await archiveButton.click({ force: true });
  expect((await taskJson(page, wsId, task.id)).archivedAt).toBeNull();

  patchHold.release();
  await archivePatchDone;
  await expect(page.getByText("보관된 태스크입니다")).toBeVisible({ timeout: 15_000 });
  await expect(editor).toContainText(bodyText);
  expect(sentPersistRequests(wire).length).toBeGreaterThan(0);
});

test("failed archive persist shows error, keeps task active and restores editing", async ({
  page,
}) => {
  const persistHold = installPersistAckHold(page);
  await ensureSetup(page);
  const wire = attachCollabWire(page);
  const { wsId, task, bodyText } = await openEditableTask(page, wire, "ZT703");
  const editor = page.getByTestId("task-body").locator(".fvoci-editor .ProseMirror");
  const archiveButton = page.getByRole("button", { name: "보관", exact: true });

  await archiveButton.click();
  await expect(page.getByRole("alert").filter({ hasText: persistFailedMessage })).toBeVisible({
    timeout: 8_000,
  });
  await expect(editor).toHaveAttribute("contenteditable", "true");
  await expect(page.getByText("보관된 태스크입니다")).toHaveCount(0);
  expect((await taskJson(page, wsId, task.id)).archivedAt).toBeNull();

  persistHold.release();
  await page.reload();
  await expect(
    page.getByTestId("task-body").locator('[data-collab-status="connected"]'),
  ).toBeVisible({
    timeout: 30_000,
  });
  await archiveButton.click();
  await expect(page.getByText("보관된 태스크입니다")).toBeVisible({ timeout: 15_000 });
  await expect(editor).toContainText(bodyText);
});
