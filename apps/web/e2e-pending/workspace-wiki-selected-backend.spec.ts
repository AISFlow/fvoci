/**
 * One identical current Vue flow for the root's isolated PostgreSQL/SQLite
 * normal-main runs. The root owns preparation, fresh dist/native binaries,
 * backend/role/FK witnesses, port-zero launch, resources and CI registration.
 * This imports base Playwright, not the PG-only collabApp server fixture.
 * The selected SQLite actor fixture opens only the root's existing isolated
 * DB after Vue setup; its explicit inputs and fresh binary are required.
 */
import { execFileSync } from "node:child_process";
import { lstatSync } from "node:fs";
import { isAbsolute } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import * as Y from "yjs";
import type { components } from "../src/generated/api";
import { watchCspViolations } from "../e2e/helpers";
import {
  admin,
  attachCollabWire,
  closeCollabContext,
  editorShape,
  ensureInstanceSetup,
  expectAwarenessTokenNotSession,
  expectMatchingPersistAck,
  expectNotDurablySaved,
  expectTokens,
  installCollabMember,
  login,
  member,
  newCollabContext,
  openEditor,
  persistBody,
  waitConnected,
} from "./collab-helpers";
import { SESSION_COOKIE, UUID_RE } from "./collab-wire";

type DocumentMeta = components["schemas"]["DocumentMetaResponse"];
type RevisionDetail = components["schemas"]["RevisionDetailResponse"];
type BodyResponse = components["schemas"]["BodyResponse"];
type SessionUser = components["schemas"]["SessionUserOutput"];
type CreateBody = components["schemas"]["CreateDocumentBody"];

function requiredFixtureInput(name: string): string {
  const value = process.env[name];
  if (!value?.trim()) throw new Error(`${name} is required for the selected actor fixture`);
  return value;
}

function installSelectedMember(selected: string): string | undefined {
  if (selected === "postgres") {
    installCollabMember();
    return undefined;
  }
  if (selected !== "sqlite") throw new Error("unsupported selected actor fixture");
  const binary = requiredFixtureInput("FVOCI_E2E_SELECTED_FIXTURE_BIN");
  if (!isAbsolute(binary) || !lstatSync(binary).isFile()) {
    throw new Error("selected fixture binary must be an explicit absolute regular file");
  }
  // Pass only the owned test DB, synthetic actor and matching test keyring.
  // Do not inherit operator DB URLs, account credentials or global env writes.
  const user = execFileSync(binary, [], {
    env: {
      PATH: process.env.PATH,
      LANG: process.env.LANG,
      E2E_DATABASE_BACKEND: "sqlite",
      FVOCI_E2E_SQLITE_RUN_ROOT: requiredFixtureInput("FVOCI_E2E_SQLITE_RUN_ROOT"),
      FVOCI_E2E_SQLITE_PATH: requiredFixtureInput("FVOCI_E2E_SQLITE_PATH"),
      PASSWORD_PEPPER_KEYS: requiredFixtureInput("PASSWORD_PEPPER_KEYS"),
      PASSWORD_PEPPER_ACTIVE_KEY_ID: requiredFixtureInput("PASSWORD_PEPPER_ACTIVE_KEY_ID"),
      E2E_USER_EMAIL: member.email,
      E2E_USER_PASSWORD: member.password,
      E2E_USER_GIVEN_NAME: member.givenName,
      E2E_USER_FAMILY_NAME: member.familyName,
      E2E_WORKSPACE_SLUG: admin.workspaceSlug,
      E2E_MEMBERSHIP_ROLE: "member",
    },
    encoding: "utf8",
    stdio: "pipe",
  }).trim();
  expect(user).toMatch(UUID_RE);
  return user;
}

async function currentUser(page: Page): Promise<SessionUser> {
  const response = await page.request.get("/api/v1/auth/me");
  expect(response.status()).toBe(200);
  const user = (await response.json()) as SessionUser;
  expect(user.userId).toMatch(UUID_RE);
  expect(user.sessionId).toMatch(UUID_RE);
  return user;
}

async function body(page: Page, path: string): Promise<BodyResponse> {
  const response = await page.request.get(`${path}/body`);
  expect(response.status()).toBe(200);
  return (await response.json()) as BodyResponse;
}

async function revision(page: Page, path: string, id: string): Promise<RevisionDetail> {
  const response = await page.request.get(`${path}/revisions/${id}`);
  expect(response.status()).toBe(200);
  const detail = (await response.json()) as RevisionDetail;
  expect(detail.id).toBe(id);
  return detail;
}

function documentNodeIds(value: unknown): string[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const node = value as { attrs?: { id?: unknown }; content?: unknown[] };
  const own = typeof node.attrs?.id === "string" ? [node.attrs.id] : [];
  return [...own, ...(node.content ?? []).flatMap(documentNodeIds)];
}

function expectRetainedNative(detail: RevisionDetail, retained: string, removed: string): void {
  // This is a read-only oracle using maintained Yjs, never an editor reset,
  // JSON reseed, native parser, or server-side JavaScript fallback.
  const update = Uint8Array.from(Buffer.from(detail.ySnapshot, "base64"));
  expect(update.byteLength).toBeGreaterThan(2);
  const decoded = Y.decodeUpdate(update);
  expect(decoded.ds.clients.size, "the native snapshot retains the deletion set").toBeGreaterThan(
    0,
  );
  const retainedStrings = decoded.structs.flatMap((struct) =>
    struct instanceof Y.Item && struct.content instanceof Y.ContentString
      ? [struct.content.str]
      : [],
  );
  expect(retainedStrings.join(""), "deleted source bytes survive the native history").toContain(
    removed,
  );
  const native = new Y.Doc({ gc: false });
  try {
    Y.applyUpdate(native, update);
    const xml: unknown = native.getXmlFragment("prosemirror").toJSON();
    if (typeof xml !== "string") throw new Error("native XML serialization must be a string");
    expect(xml).toContain(retained);
    expect(xml).not.toContain(removed);
    const ids = documentNodeIds(detail.contentJson);
    expect(ids.length).toBeGreaterThan(0);
    for (const id of ids) expect(xml).toContain(id);
  } finally {
    native.destroy();
  }
}

test("selected normal main: Vue setup, stable wiki create, native persist, manual revision and fresh actor readback", async ({
  browser,
  baseURL,
}, testInfo) => {
  const selected = process.env.FVOCI_E2E_SELECTED_BACKEND;
  expect(selected, "root must identify the actual isolated selected-backend run").toMatch(
    /^(postgres|sqlite)$/,
  );
  if (!baseURL) throw new Error("root-provided selected normal-main baseURL is required");
  testInfo.annotations.push({ type: "selected-backend", description: selected ?? "missing" });
  const ctxA = await newCollabContext(browser, baseURL);
  const ctxB = await newCollabContext(browser, baseURL);
  const pageA = await ctxA.newPage();
  const pageB = await ctxB.newPage();
  const wireA = attachCollabWire(pageA);
  const wireB = attachCollabWire(pageB);
  const cspA = watchCspViolations(pageA);
  const cspB = watchCspViolations(pageB);
  let failed = true;
  try {
    const initialSetup = await pageA.request.get("/api/v1/setup");
    expect(initialSetup.status()).toBe(200);
    expect(await initialSetup.json()).toMatchObject({ needed: true });
    await ensureInstanceSetup(pageA);
    await expect(pageA.locator("#root[data-v-app]")).toHaveCount(1);
    await login(pageA, admin.email, admin.password);
    const creator = await currentUser(pageA);
    const cookieA = (await ctxA.cookies()).find((cookie) => cookie.name === SESSION_COOKIE);
    expect(cookieA?.httpOnly).toBe(true);
    expect(cookieA?.sameSite).toBe("Lax");
    const workspaces = await pageA.request.get("/api/v1/me/workspaces");
    expect(workspaces.status()).toBe(200);
    const workspace = (
      (await workspaces.json()) as components["schemas"]["WorkspaceListResponse"]
    ).items.find((item) => item.slug === admin.workspaceSlug);
    if (!workspace) throw new Error("real setup workspace missing");
    expect(workspace.role).toBe("owner");
    const documents = `/api/v1/workspaces/${workspace.id}/documents`;
    await pageA.goto(`/w/${admin.workspaceSlug}/wiki`);
    await expect(pageA.locator("#root[data-v-app]")).toHaveCount(1);
    const sent = pageA.waitForRequest(
      (request) => request.method() === "POST" && new URL(request.url()).pathname === documents,
    );
    const created = pageA.waitForResponse(
      (response) =>
        response.request().method() === "POST" && new URL(response.url()).pathname === documents,
    );
    await pageA.getByRole("button", { name: "새 문서", exact: true }).click();
    const command = (await sent).postDataJSON() as CreateBody;
    expect(command.commandId).toMatch(UUID_RE);
    expect(command.parentId).toBeNull();
    expect(command.title).toBe("제목 없음");
    const createResponse = await created;
    expect(createResponse.status()).toBe(201);
    const meta = (await createResponse.json()) as DocumentMeta;
    expect(meta.id).toMatch(UUID_RE);
    expect(meta.createdBy).toBe(creator.userId);
    expect(meta.workspaceId).toBe(workspace.id);
    expect(meta.projectId).toBeNull();
    const displayId = `WIKI-${String(meta.number)}`;
    expect(meta.displayId).toBe(displayId);
    // Replays use exactly the command emitted by the current WikiPage.
    for (let retry = 0; retry < 2; retry++) {
      const replay = await pageA.request.post(documents, { data: command });
      expect(replay.status()).toBe(201);
      expect(await replay.json()).toEqual(meta);
    }
    const mismatch = await pageA.request.post(documents, {
      data: { ...command, title: "changed hash" },
    });
    expect(mismatch.status()).toBe(409);
    const documentPath = `${documents}/${meta.id}`;
    const editorA = await openEditor(pageA, `/w/${admin.workspaceSlug}/${displayId}`);
    const retained = "동일한 저장 문장 한글😀";
    const removed = "delete me";
    await editorA.click();
    await pageA.keyboard.type(`${retained} ${removed}`);
    await expectNotDurablySaved(pageA);
    await persistBody(pageA);
    const firstAck = await expectMatchingPersistAck(pageA, wireA);
    // A deletion and a later edit cannot be made clean by the old matching ACK.
    await editorA.focus();
    await pageA.keyboard.press("ControlOrMeta+End");
    for (let index = 0; index < removed.length; index++) await pageA.keyboard.press("Backspace");
    await pageA.keyboard.type("새 편집");
    await expectNotDurablySaved(pageA);
    await persistBody(pageA);
    const finalAck = await expectMatchingPersistAck(pageA, wireA);
    expect(finalAck).not.toBe(firstAck);
    const persisted = await body(pageA, documentPath);
    expect(JSON.stringify(persisted.contentJson)).toContain(retained);
    expect(JSON.stringify(persisted.contentJson)).toContain("새 편집");
    expect(JSON.stringify(persisted.contentJson)).not.toContain(removed);
    expect(persisted.version).toBeGreaterThan(0);
    const beforeRevision = await editorShape(pageA);
    const savedRevision = pageA.waitForResponse(
      (response) =>
        response.request().method() === "POST" &&
        new URL(response.url()).pathname === `${documentPath}/revisions`,
    );
    await pageA.getByTestId("revision-history").click();
    await pageA.getByTestId("revision-save").click();
    const revisionResponse = await savedRevision;
    expect(revisionResponse.status()).toBe(201);
    const revisionId = (
      (await revisionResponse.json()) as components["schemas"]["RevisionCreateResponse"]
    ).id;
    expect(revisionId).toMatch(UUID_RE);
    const saved = await revision(pageA, documentPath, revisionId);
    expect(saved).toMatchObject({
      targetId: meta.id,
      targetKind: "document",
      reason: "manual",
      createdBy: creator.userId,
      restoredFromId: null,
    });
    expect(saved.contentJson).toEqual(persisted.contentJson);
    expectRetainedNative(saved, retained, removed);

    const fixtureUser = installSelectedMember(selected ?? "missing");
    await login(pageB, member.email, member.password);
    const reader = await currentUser(pageB);
    expect(reader.userId).not.toBe(creator.userId);
    if (fixtureUser) expect(reader.userId).toBe(fixtureUser);
    expect(reader.sessionId).not.toBe(creator.sessionId);
    const cookieB = (await ctxB.cookies()).find((cookie) => cookie.name === SESSION_COOKIE);
    expect(cookieB?.httpOnly).toBe(true);
    expect(cookieB?.value).not.toBe(cookieA?.value);
    const wrongActor = await pageB.request.post(documents, { data: command });
    expect(wrongActor.status()).toBe(409);
    await openEditor(pageB, `/w/${admin.workspaceSlug}/${displayId}`);
    await expect(pageB.locator("#root[data-v-app]")).toHaveCount(1);
    await expectTokens(pageB, [retained, "새 편집"]);
    await expectAwarenessTokenNotSession(wireB, cookieB?.value ?? "missing");
    await expect(pageB.getByRole("button", { name: "저장", exact: true })).toBeEnabled();
    expect(await editorShape(pageB)).toEqual(beforeRevision);
    expect(await body(pageB, documentPath)).toEqual(persisted);
    const readMeta = await pageB.request.get(documentPath);
    expect(readMeta.status()).toBe(200);
    expect(await readMeta.json()).toMatchObject({
      id: meta.id,
      number: meta.number,
      workspaceId: workspace.id,
      path: meta.path,
      parentId: null,
      projectId: null,
      createdBy: creator.userId,
    });
    const freshRevision = await revision(pageB, documentPath, revisionId);
    expect(freshRevision).toEqual(saved);
    expectRetainedNative(freshRevision, retained, removed);
    // A new page/socket in the independent context reads durable state again.
    await pageB.reload();
    await waitConnected(pageB);
    await expectTokens(pageB, [retained, "새 편집"]);
    expect(await editorShape(pageB)).toEqual(beforeRevision);
    expect(await revision(pageB, documentPath, revisionId)).toEqual(saved);
    expect(cspA).toEqual([]);
    expect(cspB).toEqual([]);
    await testInfo.attach("selected-vue-native-readback.json", {
      contentType: "application/json",
      body: Buffer.from(
        JSON.stringify(
          {
            selected,
            workspaceId: workspace.id,
            document: meta,
            command,
            firstAck,
            finalAck,
            persisted,
            revision: saved,
            creatorId: creator.userId,
            freshActorId: reader.userId,
          },
          null,
          2,
        ),
      ),
    });
    failed = false;
  } finally {
    await Promise.all([closeCollabContext(ctxA, failed), closeCollabContext(ctxB, failed)]);
  }
});
