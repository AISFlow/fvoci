import assert from "node:assert/strict";
import { expect, test, type Page, type Response, type WebSocketRoute } from "@playwright/test";
import { createEncoder, toUint8Array, writeVarString, writeVarUint } from "lib0/encoding";
import { z } from "zod";
import { decodeHocuspocusFrame, frameBytes, persistParts } from "../e2e-pending/collab-wire";
import { login } from "./helpers";

test.describe.configure({ mode: "serial" });
const account = { email: "Admin@Example.COM", password: "supersecret1", slug: "revision-lifetime" };
const workspaceSchema = z.object({
  items: z.array(z.object({ id: z.string(), slug: z.string() })),
});
const projectSchema = z.object({ id: z.string(), rootDocumentId: z.string().nullable() });
const documentSchema = z.object({ id: z.string(), displayId: z.string() });
const revisionsSchema = z.object({
  items: z.array(
    z.object({
      id: z.string().uuid(),
      reason: z.string(),
      createdBy: z.string().uuid().nullable(),
    }),
  ),
});
const systemSessionHistorySchema = z.array(
  z.object({ id: z.string().uuid(), reason: z.literal("session"), createdBy: z.null() }),
);
const revisionDetailSchema = revisionsSchema.shape.items.element.extend({
  contentJson: z.unknown(),
  ySnapshot: z.string(),
});
const bodySchema = z.object({ contentJson: z.unknown() });
const meSchema = z.object({ userId: z.string(), sessionId: z.string() });

async function setup(page: Page) {
  await page.goto("/");
  await expect(
    page
      .getByRole("button", { name: "시작하기" })
      .or(page.getByRole("button", { name: "로그인", exact: true }))
      .or(page.getByRole("button", { name: "로그아웃" })),
  ).toBeVisible();
  if (await page.getByRole("button", { name: "시작하기" }).count()) {
    await page.getByLabel("성").fill("김");
    await page.getByLabel("이름", { exact: true }).fill("관리자");
    await page.getByLabel("이메일").fill(account.email);
    await page.getByLabel("비밀번호").fill(account.password);
    await page.getByLabel("워크스페이스 이름").fill("Revision lifetime");
    await page.getByLabel("주소(영문)").fill(account.slug);
    await page.getByRole("button", { name: "시작하기" }).click();
    await expect(page).toHaveURL(/\/$/);
  } else await login(page, account.email, account.password);
  const response = await page.request.get("/api/v1/me/workspaces");
  expect(response.ok()).toBe(true);
  const workspace = workspaceSchema.parse(await response.json()).items[0];
  assert.ok(workspace);
  return workspace;
}
async function createTarget(page: Page, key: string) {
  const workspace = await setup(page);
  const response = await page.request.post(`/api/v1/workspaces/${workspace.id}/projects`, {
    data: { key, name: "Revision lifetime " + key, visibility: "private" },
  });
  expect(response.status()).toBe(201);
  const project = projectSchema.parse(await response.json());
  const documents = [];
  for (const title of ["A", "B"]) {
    const created = await page.request.post(
      `/api/v1/workspaces/${workspace.id}/projects/${project.id}/documents`,
      {
        data: { parentId: project.rootDocumentId, title: "Revision " + title },
      },
    );
    expect(created.status()).toBe(201);
    documents.push(documentSchema.parse(await created.json()));
  }
  const [a, b] = documents;
  assert.ok(a && b);
  const path = (id: string) =>
    `/api/v1/workspaces/${workspace.id}/projects/${project.id}/documents/${id}`;
  return {
    a,
    b,
    path,
    projectPath: `/api/v1/workspaces/${workspace.id}/projects/${project.id}`,
    href: (displayId: string) => `/w/${workspace.slug}/${displayId}`,
  };
}

// Real Rust traffic passes through. Only the browser-facing persist ACK is
// held, dropped, or replaced; the Rust room/DB and HTTP revisions are real.
async function interceptAck(page: Page, mode: "hold" | "fail" = "hold") {
  const held: { id: string; message: string | Buffer; socket: WebSocketRoute }[] = [];
  const requests: string[] = [];
  const received: { id: string; kind: string }[] = [];
  await page.routeWebSocket(/\/collab(?:\?|$)/, (socket) => {
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      const frame = decodeHocuspocusFrame(frameBytes(message));
      if (frame?.kind === "stateless") {
        const parts = persistParts(frame.payload);
        if (parts?.kind === "request") requests.push(parts.id);
      }
      server.send(message);
    });
    server.onMessage((message) => {
      const frame = decodeHocuspocusFrame(frameBytes(message));
      const parts = frame?.kind === "stateless" ? persistParts(frame.payload) : null;
      if (parts) received.push(parts);
      if (parts?.kind === "done" && frame?.kind === "stateless") {
        if (mode === "hold") held.push({ id: parts.id, message, socket });
        else {
          // Use the installed provider's lib0 writer, without a second codec.
          const encoder = createEncoder();
          writeVarString(encoder, frame.routingKey);
          writeVarUint(encoder, 5);
          writeVarString(encoder, `persist-failed:${parts.id}`);
          socket.send(Buffer.from(toUint8Array(encoder)));
        }
      } else socket.send(message);
    });
  });
  return {
    held,
    requests,
    received,
    release(index: number) {
      const ack = held[index];
      assert.ok(ack, "a real Rust persist ACK must have arrived before release");
      ack.socket.send(ack.message);
    },
  };
}
async function edit(page: Page, href: string, text: string) {
  await page.goto(href);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const editor = page.locator(".fvoci-editor .ProseMirror");
  await expect(editor).toBeVisible();
  await editor.click();
  await page.keyboard.type(text);
  await page.getByTestId("revision-history").click();
}
async function history(page: Page, base: string) {
  const response = await page.request.get(base + "/revisions");
  expect(response.ok()).toBe(true);
  return revisionsSchema.parse(await response.json()).items;
}

test("delayed durable ACK serializes repeated revision saves and fresh clients read the last edit", async ({
  page,
  browser,
}, info) => {
  const target = await createTarget(page, "RAL1");
  const gate = await interceptAck(page);
  const plain = "latest edit before delayed revision ACK ";
  const text = plain + "😀";
  await edit(page, target.href(target.a.displayId), text);
  await page.getByTestId("revision-save").click();
  await expect.poll(() => gate.held.length).toBe(1);
  await expect(page.getByTestId("revision-save")).toBeDisabled();
  await page.getByTestId("revision-save").evaluate((button) => {
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  expect(gate.requests).toHaveLength(1);
  expect(await history(page, target.path(target.a.id))).toHaveLength(0);
  gate.release(0);
  await expect(page.getByTestId("revision-item")).toHaveCount(1);
  const [revision] = await history(page, target.path(target.a.id));
  assert.ok(revision);
  // Positive authorization control: this live actor can create a manual row,
  // and the system-only history oracle must reject that actual server row.
  expect(revision.reason).toBe("manual");
  const me = meSchema.parse(await (await page.request.get("/api/v1/auth/me")).json());
  expect(revision.createdBy).toBe(me.userId);
  expect(systemSessionHistorySchema.safeParse([revision]).success).toBe(false);
  const detail = await page.request.get(target.path(target.a.id) + "/revisions/" + revision.id);
  expect(detail.ok()).toBe(true);
  expect(bodySchema.parse(await detail.json()).contentJson).toMatchObject({
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: plain },
          { type: "emoji", attrs: { name: "grinning" } },
        ],
      },
    ],
  });
  const context = await browser.newContext({
    baseURL: new URL(page.url()).origin,
    storageState: await page.context().storageState(),
  });
  try {
    const fresh = await context.newPage();
    await fresh.goto(target.href(target.a.displayId));
    await expect(fresh.locator(".fvoci-editor .ProseMirror")).toContainText(text);
  } finally {
    await context.close();
  }
  await info.attach("real-rust-delayed-ack", {
    body: JSON.stringify({
      requests: gate.requests,
      received: gate.received,
      revision,
    }),
    contentType: "application/json",
  });
});

test("lost ACK is explicit failure and a late ACK cannot complete the next revision save", async ({
  page,
}) => {
  const target = await createTarget(page, "RAL2");
  const gate = await interceptAck(page);
  await edit(page, target.href(target.a.displayId), "timeout then retry body");
  await page.getByTestId("revision-save").click();
  await expect.poll(() => gate.held.length).toBe(1);
  // The product's existing 5s persist timeout is exercised, not overridden.
  await expect(
    page.locator(".document-revision-panel__notice").filter({ hasText: "실패" }),
  ).toBeVisible({ timeout: 7_000 });
  expect(await history(page, target.path(target.a.id))).toHaveLength(0);
  await page.getByTestId("revision-save").click();
  await expect.poll(() => gate.held.length).toBe(2);
  gate.release(0);
  await expect(page.getByTestId("revision-save")).toBeDisabled();
  expect(await history(page, target.path(target.a.id))).toHaveLength(0);
  gate.release(1);
  await expect(page.getByTestId("revision-item")).toHaveCount(1);
  expect(gate.requests).toHaveLength(2);
});

test("failed ACK refuses revision creation even when Rust committed the edit", async ({ page }) => {
  const target = await createTarget(page, "RAL3");
  const gate = await interceptAck(page, "fail");
  const text = "durable edit with failed browser ACK";
  await edit(page, target.href(target.a.displayId), text);
  await page.getByTestId("revision-save").click();
  await expect(
    page.locator(".document-revision-panel__notice").filter({ hasText: "실패" }),
  ).toBeVisible();
  expect(gate.received.some((ack) => ack.kind === "done")).toBe(true);
  expect(await history(page, target.path(target.a.id))).toHaveLength(0);
  const body = await page.request.get(target.path(target.a.id) + "/body");
  expect(body.ok()).toBe(true);
  expect(JSON.stringify(bodySchema.parse(await body.json()).contentJson)).toContain(text);
});

test("unmount and document A to B to A discard an old durable ACK without new revision or notice", async ({
  page,
}) => {
  const target = await createTarget(page, "RAL4");
  const gate = await interceptAck(page);
  const created: string[] = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/revisions"))
      created.push(request.url());
  });
  await edit(page, target.href(target.a.displayId), "persisted A before navigation");
  await page.getByTestId("revision-save").click();
  await expect.poll(() => gate.held.length).toBe(1);
  await page.goto(target.href(target.b.displayId));
  await expect(page.locator(".fvoci-editor .ProseMirror")).toBeVisible();
  gate.release(0);
  expect(await history(page, target.path(target.b.id))).toHaveLength(0);
  await page.goto(target.href(target.a.displayId));
  await expect(page.locator(".fvoci-editor .ProseMirror")).toContainText(
    "persisted A before navigation",
  );
  await page.getByTestId("revision-history").click();
  await expect(
    page.locator(".document-revision-panel__notice").filter({ hasText: "실패" }),
  ).toHaveCount(0);
  const revisions = await history(page, target.path(target.a.id));
  expect(revisions.filter((revision) => revision.reason === "manual")).toHaveLength(0);
  expect(created).toHaveLength(0);
  // Last-client departure may legitimately capture an automatic session revision.
  // A retired manual save must neither create nor promote that history entry.
  expect(revisions.every((revision) => revision.reason === "session")).toBe(true);
  systemSessionHistorySchema.parse(revisions);
});

test("actual project write revocation after durable ACK refuses revision creation", async ({
  page,
}, info) => {
  const target = await createTarget(page, "RAL5");
  const gate = await interceptAck(page);
  const text = "edit before project archive";
  const base = target.path(target.a.id);
  const browserCreates: Response[] = [];
  page.on("response", (response) => {
    if (response.request().method() === "POST" && response.url().endsWith(base + "/revisions"))
      browserCreates.push(response);
  });
  await edit(page, target.href(target.a.displayId), text);
  await page.getByTestId("revision-save").click();
  await expect.poll(() => gate.held.length).toBe(1);
  const archived = await page.request.post(target.projectPath + "/archive");
  expect(archived.status()).toBe(200);
  gate.release(0);
  await expect(
    page.locator(".document-revision-panel__notice").filter({ hasText: "실패" }),
  ).toBeVisible();
  const beforeDeparture = await history(page, base);
  expect(beforeDeparture.filter((revision) => revision.reason === "manual")).toHaveLength(0);
  systemSessionHistorySchema.parse(beforeDeparture);
  const body = await page.request.get(base + "/body");
  expect(body.ok()).toBe(true);
  const durableContent = bodySchema.parse(await body.json()).contentJson;
  expect(JSON.stringify(durableContent)).toContain(text);

  // Last departure is real and deterministic. An archived, still-live project
  // permits system history of the already committed edit, never a manual save.
  await page.goto("/");
  await expect.poll(async () => (await history(page, base)).length).toBe(1);
  const before = await history(page, base);
  const [session] = systemSessionHistorySchema.parse(before);
  assert.ok(session);
  const detail = await page.request.get(base + "/revisions/" + session.id);
  expect(detail.ok()).toBe(true);
  const snapshot = revisionDetailSchema.parse(await detail.json());
  expect(snapshot).toMatchObject(session);
  expect(snapshot.contentJson).toEqual(durableContent);
  expect(snapshot.ySnapshot).not.toBe("");

  const refused = await page.request.post(base + "/revisions");
  expect(refused.status()).toBe(409);
  expect(z.object({ code: z.string() }).parse(await refused.json()).code).toBe("project_archived");
  // The refused request must neither insert a manual row nor promote/mutate
  // the automatic head, even though it contains the same durable content.
  expect(await history(page, base)).toEqual(before);
  const unchanged = await page.request.get(base + "/revisions/" + session.id);
  expect(unchanged.ok()).toBe(true);
  expect(revisionDetailSchema.parse(await unchanged.json())).toEqual(snapshot);
  // The native room may retire before a UI POST starts. Every response that
  // does arrive must be a real denial; the explicit POST above always runs.
  const browserRefusals = [];
  for (const response of browserCreates) {
    expect(response.status()).toBe(409);
    const code = z.object({ code: z.string() }).parse(await response.json()).code;
    expect(code).toBe("project_archived");
    browserRefusals.push({ status: response.status(), code });
  }
  await info.attach("archived-system-revision", {
    body: JSON.stringify({
      base,
      snapshot,
      durableContent,
      refusedStatus: refused.status(),
      browserRefusals,
    }),
    contentType: "application/json",
  });
});

test("actual logout retires the old session and same-user reentry cannot consume its late ACK", async ({
  page,
}) => {
  const target = await createTarget(page, "RAL6");
  const before = await page.request.get("/api/v1/auth/me");
  expect(before.status()).toBe(200);
  const oldSession = meSchema.parse(await before.json());
  const gate = await interceptAck(page);
  const created: string[] = [];
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/revisions"))
      created.push(request.url());
  });
  await edit(page, target.href(target.a.displayId), "body persisted before real logout");
  await page.getByTestId("revision-save").click();
  await expect.poll(() => gate.held.length).toBe(1);
  const loggedOut = await page.request.post("/api/v1/auth/logout");
  expect(loggedOut.status()).toBe(204);
  expect((await page.request.get("/api/v1/auth/me")).status()).toBe(401);
  await login(page, account.email, account.password);
  const after = await page.request.get("/api/v1/auth/me");
  expect(after.status()).toBe(200);
  const newSession = meSchema.parse(await after.json());
  expect(newSession.userId).toBe(oldSession.userId);
  expect(newSession.sessionId).not.toBe(oldSession.sessionId);
  await page.goto(target.href(target.a.displayId));
  await expect(page.locator(".fvoci-editor .ProseMirror")).toContainText(
    "body persisted before real logout",
  );
  gate.release(0);
  const revisions = await history(page, target.path(target.a.id));
  expect(revisions.filter((revision) => revision.reason === "manual")).toHaveLength(0);
  expect(created).toHaveLength(0);
  systemSessionHistorySchema.parse(revisions);
});
