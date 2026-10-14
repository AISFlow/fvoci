/** Explicit config only; the original normal ON1/restart1/OFF8 is untouched.
 * The caller owns normal installation, SMTP, roles, native/dist identity and
 * cleanup. This invocation owns a fresh auth actor allocation; no business-row SQL. */
import {
  expect,
  test,
  type Browser,
  type BrowserContext,
  type Page,
  type TestInfo,
} from "@playwright/test";
import type { components } from "../src/generated/api";
import { waitForCapturedMail } from "../e2e/helpers";
import {
  admin,
  member,
  attachCollabWire,
  createWikiDoc,
  expectMatchingPersistAck,
  login,
  newCollabContext,
  openEditor,
  persistBody,
  sentPersistRequests,
  ensureInstanceSetup,
  type CollabWireLog,
} from "./collab-helpers";
import { UUID_RE } from "./collab-wire";
import {
  activeRoom,
  currentSession,
  documentObservation,
  holdSave,
  rejectedCookie,
  type AuthActor,
  type AuthFlow,
  type AuthTarget,
} from "./auth-session-helpers";
import { AUTH_CASES, assertAuthNoEffect } from "./auth-session-contract";
import { installSelectedMember, selectedSetupNeeded } from "./selected-backend-fixture";

type Schema = components["schemas"];
const changedPassword = "auth-changed-secret1";
const resetPassword = "auth-reset-secret1";
let owner: AuthActor, a: AuthActor, b: AuthActor;
let base: string, flow: AuthFlow;
let ownerWire: CollabWireLog;
const contexts: BrowserContext[] = [];

async function actor(
  browser: Browser,
  credentials: { email: string; password: string },
): Promise<AuthActor> {
  const context = await newCollabContext(browser, base);
  contexts.push(context);
  const page = await context.newPage();
  await login(page, credentials.email, credentials.password);
  return { context, page, user: await currentSession(page) };
}

test.beforeAll(async ({ browser, baseURL }) => {
  if (!baseURL) throw new Error("Allocated auth normal server is required");
  base = baseURL;
  const selectedFlow = process.env.FVOCI_E2E_SELECTED_FLOW;
  if (selectedFlow !== "on" && selectedFlow !== "off") throw new Error("Auth flow is required");
  flow = selectedFlow;
  const selected = process.env.FVOCI_E2E_SELECTED_BACKEND;
  if (!selected) throw new Error("The selected auth backend is required");
  const setupContext = await newCollabContext(browser, base);
  contexts.push(setupContext);
  const setup = await setupContext.newPage();
  const initial = await setup.request.get("/api/v1/setup");
  expect(initial.status()).toBe(200);
  expect(await initial.json()).toEqual({ needed: selectedSetupNeeded() });
  if (selectedSetupNeeded()) await ensureInstanceSetup(setup);
  owner = await actor(browser, admin);
  ownerWire = attachCollabWire(owner.page);
  expect(
    (await (await owner.page.request.get("/api/v1/setup")).json()) as { needed: boolean },
  ).toEqual({ needed: false });
  installSelectedMember(selected);
  a = await actor(browser, member);
  b = await actor(browser, member);
  expect(a.user.userId).toBe(b.user.userId);
  expect(a.user.sessionId).not.toBe(b.user.sessionId);
  expect(owner.user.userId).not.toBe(a.user.userId);
});

test.afterAll(async () => {
  const closed = await Promise.allSettled(contexts.map((context) => context.close()));
  if (closed.some((result) => result.status === "rejected"))
    throw new Error("Auth browser context closure failed");
});

async function mount(page: Page, target: AuthTarget): Promise<void> {
  if (flow === "on") {
    await openEditor(page, target.url);
    return;
  }
  await page.goto(target.url);
  await expect(page.locator('[data-body-mode="off"]')).toBeVisible();
  await expect(page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]')).toBeVisible();
}

async function saveOwner(text: string, target: AuthTarget): Promise<void> {
  const editor = owner.page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]');
  await editor.fill(text);
  if (flow === "on") {
    await persistOwner();
  } else {
    const received = owner.page.waitForResponse(
      (r) =>
        r.request().method() === "PUT" &&
        new URL(r.url()).pathname === `${target.path}/body/versioned`,
    );
    await owner.page.getByRole("button", { name: "저장", exact: true }).click();
    const response = await received;
    expect(response.status()).toBe(200);
    const command = response.request().postDataJSON() as Schema["SaveVersionedBodyInput"];
    const ack = (await response.json()) as Schema["SaveVersionedBodyResponse"];
    expect(ack.commandId).toBe(command.commandId);
    expect(ack.targetId).toBe(target.id);
    expect(ack.revisionId).toMatch(UUID_RE);
    await expect(owner.page.locator('[data-body-mode="off"]')).toHaveAttribute(
      "data-body-persisted",
      "true",
    );
  }
}

async function persistOwner(): Promise<void> {
  const prior = sentPersistRequests(ownerWire).length;
  await persistBody(owner.page);
  await expect.poll(() => sentPersistRequests(ownerWire).length).toBeGreaterThan(prior);
  await expectMatchingPersistAck(owner.page, ownerWire);
}

async function seed(title: string): Promise<AuthTarget> {
  const doc = await createWikiDoc(owner.page, title);
  const target = { ...doc, path: `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}` };
  await mount(owner.page, target);
  await owner.page
    .locator('.fvoci-editor .ProseMirror[contenteditable="true"]')
    .fill("retained authorized auth history 한글");
  if (flow === "on") {
    await persistOwner();
    expect((await owner.page.request.post(`${target.path}/revisions`)).status()).toBe(201);
  } else {
    const saved = owner.page.waitForResponse(
      (r) =>
        r.request().method() === "PUT" &&
        new URL(r.url()).pathname === `${target.path}/body/versioned`,
    );
    await owner.page.getByRole("button", { name: "저장", exact: true }).click();
    expect((await saved).status()).toBe(200);
    await expect(owner.page.locator('[data-body-mode="off"]')).toHaveAttribute(
      "data-body-persisted",
      "true",
    );
  }
  return target;
}

async function live(actor: AuthActor): Promise<void> {
  const current = await currentSession(actor.page);
  expect(current.userId).toBe(actor.user.userId);
  expect(current.sessionId).toBe(actor.user.sessionId);
}

async function control(actor: AuthActor, path: string): Promise<Page> {
  const page = await actor.context.newPage();
  await page.goto(path);
  return page;
}

async function passwordLogin(page: Page, password: string, expected: 200 | 401): Promise<void> {
  await page.goto("/login");
  await page.getByLabel("이메일").fill(member.email);
  await page.getByLabel("비밀번호", { exact: true }).fill(password);
  const received = page.waitForResponse(
    (r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/api/v1/auth/login",
  );
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  expect((await received).status()).toBe(expected);
  if (expected === 200) await expect(page).toHaveURL(/\/$/);
  else await expect(page.getByRole("alert")).toBeVisible();
}

async function witness(info: TestInfo, input: Record<string, unknown>): Promise<void> {
  await info.attach("auth-session-document-effects.json", {
    contentType: "application/json",
    body: JSON.stringify({
      schema: 1,
      scope: "document-effects",
      backend: process.env.FVOCI_E2E_SELECTED_BACKEND,
      flow,
      source: process.env.FVOCI_E2E_AUTH_SOURCE,
      tree: process.env.FVOCI_E2E_AUTH_TREE,
      actor: a.user.userId,
      owner: owner.user.userId,
      ...input,
    }),
  });
}

test(AUTH_CASES[0], async ({ browser }, info) => {
  const target = await seed("auth explicit logout");
  const roomA = await activeRoom(a.page),
    roomB = await activeRoom(b.page);
  await mount(a.page, target);
  await mount(b.page, target);
  const before = await documentObservation(browser, base, owner, target, flow);
  const oldCookie = await a.context.cookies();
  const pending = await holdSave(a.page, target, flow);
  const logout = await control(a, "/");
  try {
    const response = logout.waitForResponse(
      (r) => r.request().method() === "POST" && new URL(r.url()).pathname === "/api/v1/auth/logout",
    );
    await logout.getByRole("button", { name: "로그아웃" }).click();
    expect((await response).status()).toBe(204);
    await expect(logout).toHaveURL(/\/login$/);
  } finally {
    pending.release();
  }
  const refusal = await pending.refused();
  if (flow === "on") {
    await expect.poll(() => roomA.closedCodes).toContain(1008);
    expect(roomA.connections).toBeGreaterThan(0);
    expect(roomB.closedCodes).not.toContain(1008);
    await expect(b.page.locator('[data-collab-status="connected"]')).toBeVisible();
  } else {
    expect(roomA.connections + roomB.connections).toBe(0);
  }
  await rejectedCookie(browser, base, oldCookie, target, flow);
  await live(b);
  await live(owner);
  const after = await documentObservation(browser, base, owner, target, flow);
  assertAuthNoEffect(before, after);
  await saveOwner("healthy owner after logout refusal", target);
  await witness(info, {
    targetId: target.id,
    before,
    after,
    refusal,
    closed: roomA.closedCodes,
    retainedSession: b.user.sessionId,
  });
});

test(AUTH_CASES[1], async ({ browser }, info) => {
  // The retained B session from logout is the password-changing session.
  a = await actor(browser, member);
  const target = await seed("auth password change");
  const roomA = await activeRoom(a.page),
    roomB = await activeRoom(b.page);
  await mount(a.page, target);
  await mount(b.page, target);
  const before = await documentObservation(browser, base, owner, target, flow);
  const settings = await control(b, "/settings/account");
  await settings.locator("#settings-current-password").fill("wrong-current-password");
  await settings.locator("#settings-new-password").fill(changedPassword);
  const wrong = settings.waitForResponse(
    (r) =>
      r.request().method() === "PATCH" && new URL(r.url()).pathname === "/api/v1/auth/password",
  );
  await settings.getByRole("button", { name: "비밀번호 변경" }).click();
  expect((await wrong).status()).toBe(400);
  await live(a);
  await live(b);
  assertAuthNoEffect(before, await documentObservation(browser, base, owner, target, flow));
  const oldCookie = await a.context.cookies();
  const pending = await holdSave(a.page, target, flow);
  try {
    await settings.locator("#settings-current-password").fill(member.password);
    const changed = settings.waitForResponse(
      (r) =>
        r.request().method() === "PATCH" && new URL(r.url()).pathname === "/api/v1/auth/password",
    );
    await settings.getByRole("button", { name: "비밀번호 변경" }).click();
    expect((await changed).status()).toBe(200);
    await expect(
      settings.getByRole("status").filter({ hasText: "비밀번호가 변경되었습니다." }),
    ).toBeVisible();
  } finally {
    pending.release();
  }
  const refusal = await pending.refused();
  if (flow === "on") {
    await expect.poll(() => roomA.closedCodes).toContain(1008);
    expect(roomB.closedCodes).not.toContain(1008);
    await expect(b.page.locator('[data-collab-status="connected"]')).toBeVisible();
  }
  await live(b);
  await live(owner);
  await rejectedCookie(browser, base, oldCookie, target, flow);
  const fresh = await newCollabContext(browser, base);
  contexts.push(fresh);
  const client = await fresh.newPage();
  await passwordLogin(client, member.password, 401);
  await passwordLogin(client, changedPassword, 200);
  a = { context: fresh, page: client, user: await currentSession(client) };
  expect(a.user.userId).toBe(b.user.userId);
  expect(a.user.sessionId).not.toBe(b.user.sessionId);
  const after = await documentObservation(browser, base, owner, target, flow);
  assertAuthNoEffect(before, after);
  await saveOwner("healthy owner after password-change refusal", target);
  await witness(info, {
    targetId: target.id,
    before,
    after,
    refusal,
    wrongCurrentStatus: 400,
    oldPasswordStatus: 401,
    newPasswordStatus: 200,
    retainedSession: b.user.sessionId,
  });
});

test(AUTH_CASES[2], async ({ browser }, info) => {
  const target = await seed("auth normal captured reset");
  const roomA = await activeRoom(a.page),
    roomB = await activeRoom(b.page);
  await mount(a.page, target);
  await mount(b.page, target);
  await live(a);
  await live(b);
  const cookiesA = await a.context.cookies(),
    cookiesB = await b.context.cookies();
  const before = await documentObservation(browser, base, owner, target, flow);
  const anonymous = await newCollabContext(browser, base);
  contexts.push(anonymous);
  const reset = await anonymous.newPage();
  await reset.goto("/login");
  await reset.getByRole("button", { name: "비밀번호를 잊으셨나요?" }).click();
  await reset.locator("#password-reset-email").fill(member.email);
  const since = Date.now() / 1000;
  const requested = reset.waitForResponse(
    (r) =>
      r.request().method() === "POST" &&
      new URL(r.url()).pathname === "/api/v1/auth/password-reset",
  );
  await reset.getByRole("button", { name: "재설정 링크 받기" }).click();
  expect((await requested).status()).toBe(202);
  const mail = await waitForCapturedMail(
    (message) =>
      message.ts >= since &&
      message.to.toLowerCase() === member.email.toLowerCase() &&
      message.text.includes("/reset-password?token="),
  );
  const token = mail.text.match(/reset-password\?token=([A-Za-z0-9_-]+)/)?.[1];
  if (!token) throw new Error("Normal SMTP capture did not contain the reset token");
  async function confirm(value: string, password: string, status: 200 | 400): Promise<void> {
    await reset.goto(`/reset-password?token=${value}`);
    await reset.getByLabel("새 비밀번호").fill(password);
    const response = reset.waitForResponse(
      (r) =>
        r.request().method() === "POST" &&
        new URL(r.url()).pathname === "/api/v1/auth/password-reset/confirm",
    );
    await reset.getByRole("button", { name: "비밀번호 변경" }).click();
    const actual = await response;
    expect(actual.status()).toBe(status);
    if (status === 400)
      expect(((await actual.json()) as { code: string }).code).toBe("magic_invalid");
  }
  await confirm((token[0] === "A" ? "B" : "A") + token.slice(1), "must-not-replace-secret1", 400);
  await live(a);
  await live(b);
  assertAuthNoEffect(before, await documentObservation(browser, base, owner, target, flow));
  const pending = await holdSave(a.page, target, flow);
  try {
    await confirm(token, resetPassword, 200);
  } finally {
    pending.release();
  }
  await expect(reset).toHaveURL(/\/login\?reset=1/);
  const refusal = await pending.refused();
  if (flow === "on") {
    await expect.poll(() => roomA.closedCodes).toContain(1008);
    await expect.poll(() => roomB.closedCodes).toContain(1008);
  }
  expect((await b.page.request.get("/api/v1/auth/me")).status()).toBe(401);
  await rejectedCookie(browser, base, cookiesA, target, flow);
  await rejectedCookie(browser, base, cookiesB, target, flow);
  await live(owner);
  await confirm(token, "must-not-replace-secret1", 400);
  const after = await documentObservation(browser, base, owner, target, flow);
  assertAuthNoEffect(before, after);
  await passwordLogin(reset, changedPassword, 401);
  await passwordLogin(reset, resetPassword, 200);
  expect((await currentSession(reset)).userId).toBe(a.user.userId);
  await saveOwner("healthy owner after reset refusal", target);
  await witness(info, {
    targetId: target.id,
    before,
    after,
    refusal,
    tamperedStatus: 400,
    spentStatus: 400,
    oldPasswordStatus: 401,
    newPasswordStatus: 200,
    revokedSessions: [a.user.sessionId, b.user.sessionId],
  });
});
