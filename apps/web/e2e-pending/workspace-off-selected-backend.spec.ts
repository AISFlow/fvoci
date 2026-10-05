/** Actual normal-main OFF flow. Root supplies a freshly initialized isolated
 * selected backend, current native producer, fresh Vue dist and port-zero URL.
 * No PG-only server fixture, business-row seed, mocked ACK or fallback backend.
 * Registration and stopped-server restart driver remain root owned. */
import { randomUUID } from "node:crypto";
import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import * as Y from "yjs";
import { yDocToTiptapJson } from "../../../packages/editor/src/collab-tiptap";
import { extractText, walkTiptap } from "../../../packages/editor/src/extract";
import type { components } from "../src/generated/api";
import {
  admin,
  member,
  ensureInstanceSetup,
  login,
  newCollabContext,
  editorShape,
} from "./collab-helpers";
import { UUID_RE } from "./collab-wire";
import { installSelectedMember, requiredFixtureInput } from "./selected-backend-fixture";

type Schema = components["schemas"];
type Body = Schema["VersionedBodyResponse"];
type SaveCommand = Schema["SaveVersionedBodyInput"];
type SaveResult = Schema["SaveVersionedBodyResponse"];
type CopyCommand = Schema["OffDraftCreateBody"];
type CopyResult = Schema["OffDraftCreateResponse"];
type Target = { id: string; path: string; url: string };
type ActorPage = {
  context: BrowserContext;
  page: Page;
  sockets: string[];
  accessStreams: { status: number; contentType: string }[];
};

let workspaceId: string;
let selected: string;
let baseUrl: string;
let memberId: string;

async function actor(browser: Browser, who: typeof admin | typeof member): Promise<ActorPage> {
  const context = await newCollabContext(browser, baseUrl);
  const page = await context.newPage();
  const sockets: string[] = [];
  const accessStreams: { status: number; contentType: string }[] = [];
  page.on("response", (response) => {
    if (new URL(response.url()).pathname.endsWith("/access-stream"))
      accessStreams.push({
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
  });
  page.on("websocket", (socket) => sockets.push(socket.url()));
  await login(page, who.email, who.password);
  const identity = await page.request.get("/api/v1/auth/me");
  expect(identity.status()).toBe(200);
  const user = (await identity.json()) as Schema["SessionUserOutput"];
  expect(user.userId).toMatch(UUID_RE);
  expect(user.sessionId).toMatch(UUID_RE);
  return { context, page, sockets, accessStreams };
}

async function openOff(page: Page, target: Target): Promise<void> {
  const response = await page.goto(target.url);
  expect(response?.status()).toBe(200);
  await mountedOff(page);
}

async function mountedOff(page: Page): Promise<void> {
  await expect(page.locator('[data-body-mode="off"]')).toBeVisible();
  await expect(page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]')).toBeVisible();
  await expect(page.locator("[data-collab-status]")).toHaveCount(0);
  await expect(page.locator(".collaboration-carets__caret")).toHaveCount(0);
}

async function wikiNavigate(page: Page, target: Target): Promise<void> {
  await page.locator(`a[href="/w/${admin.workspaceSlug}/wiki"]`).first().click();
  await expect(page).toHaveURL(`/w/${admin.workspaceSlug}/wiki`);
  await page.locator(`a[href="${target.url}"]`).click();
  await expect(page).toHaveURL(target.url);
  await mountedOff(page);
}

async function readBody(page: Page, target: Target): Promise<Body> {
  const response = await page.request.get(`${target.path}/body/versioned`);
  expect(response.status()).toBe(200);
  const value = (await response.json()) as Body;
  expect(value.targetId).toBe(target.id);
  expect(value.tailSeq).toMatch(/^(0|[1-9]\d*)$/);
  expect(value.writable).toBe(true);
  return value;
}

function nodeIds(content: unknown): string[] {
  const ids: string[] = [];
  walkTiptap(content, (node) => {
    if (typeof node.attrs?.id === "string") ids.push(node.attrs.id);
  });
  expect(ids.length).toBeGreaterThan(0);
  for (const id of ids) expect(id).toMatch(UUID_RE);
  expect(new Set(ids).size).toBe(ids.length);
  return ids;
}

function expectNative(body: Body): void {
  // Read-only decoding through the maintained editor adapter, never reseeding.
  const doc = new Y.Doc({ gc: false });
  try {
    Y.applyUpdate(doc, Buffer.from(body.snapshotV1, "base64"));
    for (const tail of body.tailV1) Y.applyUpdate(doc, Buffer.from(tail, "base64"));
    expect(yDocToTiptapJson(doc)).toEqual(body.contentJson);
    nodeIds(body.contentJson);
  } finally {
    doc.destroy();
  }
}

function expectForwardHistory(before: Body, after: Body): void {
  const vector = (body: Body) => {
    const doc = new Y.Doc({ gc: false });
    try {
      Y.applyUpdate(doc, Buffer.from(body.snapshotV1, "base64"));
      for (const tail of body.tailV1) Y.applyUpdate(doc, Buffer.from(tail, "base64"));
      return Y.decodeStateVector(Y.encodeStateVector(doc));
    } finally {
      doc.destroy();
    }
  };
  const original = vector(before);
  const current = vector(after);
  for (const [client, clock] of original) {
    expect(
      current.get(client),
      "a forward OFF save must retain each original native client history",
    ).toBeDefined();
    expect(current.get(client) ?? 0).toBeGreaterThanOrEqual(clock);
  }
}

async function replaceText(page: Page, text: string): Promise<void> {
  const editor = page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]');
  await editor.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type(text);
  await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
    "data-body-persisted",
    "false",
  );
  expect((await editorShape(page)).text).toBe(text);
}

async function save(page: Page, target: Target, expectedStatus: 200 | 409 = 200) {
  const matches = (url: string) => new URL(url).pathname === `${target.path}/body/versioned`;
  const sent = page.waitForRequest(
    (request) => request.method() === "PUT" && matches(request.url()),
  );
  const received = page.waitForResponse(
    (response) => response.request().method() === "PUT" && matches(response.url()),
  );
  await page.getByRole("button", { name: "저장", exact: true }).click();
  const command = (await sent).postDataJSON() as SaveCommand;
  expect(command.commandId).toMatch(UUID_RE);
  expect(command.expectedTailSeq).toMatch(/^(0|[1-9]\d*)$/);
  expect(command.updateV1.length).toBeGreaterThan(0);
  const response = await received;
  expect(response.status()).toBe(expectedStatus);
  if (expectedStatus === 409) {
    await expect(page.getByTestId("off-body-conflict")).toBeVisible();
    return { command, result: null };
  }
  const result = (await response.json()) as SaveResult;
  expect(result).toMatchObject({ commandId: command.commandId, targetId: target.id });
  expect(result.revisionId).toMatch(UUID_RE);
  expect(BigInt(result.tailSeq)).toBeGreaterThan(BigInt(command.expectedTailSeq));
  await expect(page.locator('[data-body-mode="off"]')).toHaveAttribute(
    "data-body-persisted",
    "true",
  );
  return { command, result };
}

function confirmed(result: SaveResult | null): SaveResult {
  if (!result) throw new Error("the actual save did not return a confirmed result");
  return result;
}

function expectRevisionNative(body: Body, snapshotBytes: Uint8Array, content: unknown): void {
  const live = new Y.Doc({ gc: false });
  let historical: Y.Doc | undefined;
  try {
    Y.applyUpdate(live, Buffer.from(body.snapshotV1, "base64"));
    for (const tail of body.tailV1) Y.applyUpdate(live, Buffer.from(tail, "base64"));
    // Revision ySnapshot is a delete-set/state-vector Snapshot, not updateV1.
    const snapshot = Y.decodeSnapshot(snapshotBytes);
    expect(Y.equalSnapshots(snapshot, Y.snapshot(live))).toBe(true);
    historical = Y.createDocFromSnapshot(live, snapshot);
    expect(yDocToTiptapJson(historical)).toEqual(content);
  } finally {
    historical?.destroy();
    live.destroy();
  }
}

function revisionOracleNegatives(body: Body, snapshotBytes: Uint8Array, content: unknown): void {
  const snapshot = Y.decodeSnapshot(snapshotBytes);
  expect(snapshot.sv.size).toBeGreaterThan(0);
  const wrongVector = new Map(snapshot.sv);
  const [client, clock] = [...wrongVector][0]!;
  wrongVector.set(client, clock + 1);
  expect(() =>
    expectRevisionNative(
      body,
      Y.encodeSnapshot(Y.createSnapshot(snapshot.ds, wrongVector)),
      content,
    ),
  ).toThrow();
  const deleted = new Y.Doc({ gc: false });
  const wrongDecoder = new Y.Doc({ gc: false });
  try {
    // Maintained Yjs creates a real delete set; no byte parser or custom CRDT.
    deleted.clientID = 353;
    deleted.getText("negative").insert(0, "history");
    deleted.getText("negative").delete(0, 1);
    const generated = Y.snapshot(deleted);
    expect(() => Y.applyUpdate(wrongDecoder, Y.encodeSnapshot(generated))).toThrow();
    const wrongDeletes = Y.equalSnapshots(Y.createSnapshot(generated.ds, snapshot.sv), snapshot)
      ? Y.createDeleteSet()
      : generated.ds;
    expect(() =>
      expectRevisionNative(
        body,
        Y.encodeSnapshot(Y.createSnapshot(wrongDeletes, snapshot.sv)),
        content,
      ),
    ).toThrow();
  } finally {
    deleted.destroy();
    wrongDecoder.destroy();
  }
  expect(() => expectRevisionNative(body, snapshotBytes, { type: "doc", content: [] })).toThrow();
  expect(() =>
    expectRevisionNative(body, Buffer.from(body.snapshotV1, "base64"), content),
  ).toThrow();
}

async function revision(page: Page, target: Target, result: SaveResult, body: Body): Promise<void> {
  const response = await page.request.get(`${target.path}/revisions/${result.revisionId}`);
  expect(response.status()).toBe(200);
  const detail = (await response.json()) as Schema["RevisionDetailResponse"];
  expect(detail).toMatchObject({
    id: result.revisionId,
    targetId: target.id,
    reason: "manual",
    contentJson: body.contentJson,
  });
  const current = await readBody(page, target);
  expect(current).toEqual(body);
  const bytes = Buffer.from(detail.ySnapshot, "base64");
  expectRevisionNative(current, bytes, body.contentJson);
  revisionOracleNegatives(current, bytes, body.contentJson);
}

async function wiki(page: Page): Promise<Target> {
  const documents = `/api/v1/workspaces/${workspaceId}/documents`;
  await page.goto(`/w/${admin.workspaceSlug}/wiki`);
  const sent = page.waitForRequest(
    (request) => request.method() === "POST" && new URL(request.url()).pathname === documents,
  );
  const received = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && new URL(response.url()).pathname === documents,
  );
  await page.getByRole("button", { name: "새 문서", exact: true }).click();
  const command = (await sent).postDataJSON() as Schema["CreateDocumentBody"];
  expect(command.commandId).toMatch(UUID_RE);
  const response = await received;
  expect(response.status()).toBe(201);
  const meta = (await response.json()) as Schema["DocumentMetaResponse"];
  expect(meta).toMatchObject({ workspaceId, projectId: null, title: "제목 없음" });
  expect(meta.id).toMatch(UUID_RE);
  const replay = await page.request.post(documents, { data: command });
  expect(replay.status()).toBe(201);
  expect(await replay.json()).toEqual(meta);
  const wrong = await page.request.post(documents, {
    data: { ...command, title: "different logical body" },
  });
  expect(wrong.status()).toBe(409);
  if (!meta.displayId) throw new Error("actual wiki display ID missing");
  return {
    id: meta.id,
    path: `${documents}/${meta.id}`,
    url: `/w/${admin.workspaceSlug}/${meta.displayId}`,
  };
}

async function compare(page: Page, start: Body, mine: string, current: Body): Promise<void> {
  const panel = page.getByTestId("off-body-conflict");
  await expect(panel).toBeVisible();
  const expected = [start.contentJson, null, current.contentJson];
  const details = panel.locator("details");
  await expect(details).toHaveCount(3);
  for (let index = 0; index < 3; index++) {
    const value: unknown = JSON.parse(await details.nth(index).locator("pre").innerText());
    if (index === 1) expect(extractText(value)).toBe(mine);
    else expect(value).toEqual(expected[index]);
  }
}

async function twoWriters(a: ActorPage, b: ActorPage, target: Target) {
  await openOff(a.page, target);
  await openOff(b.page, target);
  const start = await readBody(a.page, target);
  expect(await readBody(b.page, target)).toEqual(start);
  const winner = `winner 한글😀 ${randomUUID()}`;
  const loser = `retained mine 中 ${randomUUID()}`;
  await replaceText(a.page, winner);
  await a.page.keyboard.press("ControlOrMeta+A");
  await a.page.keyboard.press("ControlOrMeta+B");
  await a.page.keyboard.press("ControlOrMeta+End");
  expect((await editorShape(a.page)).bold).toEqual([winner]);
  await replaceText(b.page, loser);
  const committed = await save(a.page, target);
  expect(committed.command.expectedTailSeq).toBe(start.tailSeq);
  const current = await readBody(a.page, target);
  expect(extractText(current.contentJson)).toBe(winner);
  const bold: string[] = [];
  walkTiptap(current.contentJson, (node) => {
    if (
      node.type === "text" &&
      Array.isArray(node.marks) &&
      node.marks.some(
        (mark: unknown) =>
          !!mark && typeof mark === "object" && "type" in mark && mark.type === "bold",
      )
    ) {
      if (typeof node.text !== "string") throw new Error("committed bold text must be a string");
      bold.push(node.text);
    }
  });
  expect(bold.join("")).toBe(winner);
  expectNative(current);
  expectForwardHistory(start, current);
  expect(current.tailSeq).toBe(committed.result?.tailSeq);
  await revision(a.page, target, confirmed(committed.result), current);
  const rejected = await save(b.page, target, 409);
  expect(rejected.command.expectedTailSeq).toBe(start.tailSeq);
  expect(rejected.command.commandId).not.toBe(committed.command.commandId);
  expect(await readBody(a.page, target)).toEqual(current);
  await compare(b.page, start, loser, current);
  expect((await editorShape(b.page)).text).toBe(loser);
  expect(a.sockets).toEqual([]);
  expect(b.sockets).toEqual([]);
  return { start, loser, current, committed, rejected };
}

test.describe("selected normal main OFF", () => {
  test.beforeAll(async ({ browser, baseURL }) => {
    selected = requiredFixtureInput("FVOCI_E2E_SELECTED_BACKEND");
    expect(selected).toMatch(/^(postgres|sqlite)$/);
    baseUrl = requiredFixtureInput("PLAYWRIGHT_BASE_URL");
    expect(baseURL).toBe(baseUrl);
    const context = await newCollabContext(browser, baseUrl);
    const page = await context.newPage();
    try {
      const initial = await page.request.get("/api/v1/setup");
      expect(initial.status()).toBe(200);
      expect(await initial.json()).toMatchObject({ needed: true, realtimeMode: "off" });
      await ensureInstanceSetup(page);
      await login(page, admin.email, admin.password);
      const response = await page.request.get("/api/v1/me/workspaces");
      expect(response.status()).toBe(200);
      const workspaces = (await response.json()) as Schema["WorkspaceListResponse"];
      const workspace = workspaces.items.find((item) => item.slug === admin.workspaceSlug);
      if (!workspace) throw new Error("real Vue setup workspace missing");
      expect(workspace.role).toBe("owner");
      workspaceId = workspace.id;
      installSelectedMember(selected);
      await login(page, member.email, member.password);
      const me = await page.request.get("/api/v1/auth/me");
      expect(me.status()).toBe(200);
      memberId = ((await me.json()) as Schema["SessionUserOutput"]).userId;
      expect(memberId).toMatch(UUID_RE);
      const collab = await page.request.get("/collab");
      expect(collab.status()).toBe(503);
      expect(await collab.json()).toEqual({
        type: "about:blank",
        title: "collaboration unavailable",
        status: 503,
        code: "collab_unavailable",
      });
      const upgrade = await page.request.get("/collab", {
        headers: {
          Upgrade: "websocket",
          Connection: "Upgrade",
          "Sec-WebSocket-Version": "13",
          "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
          Origin: baseUrl,
        },
      });
      expect(upgrade.status()).toBe(503);
      expect(await upgrade.json()).toMatchObject({ code: "collab_unavailable", status: 503 });
    } finally {
      await context.close();
    }
  });

  test("wiki: actual two-writer CAS, start/mine/current, manual resolution and fresh-client native history", async ({
    browser,
  }) => {
    const a = await actor(browser, admin);
    const b = await actor(browser, member);
    const fresh = await actor(browser, member);
    try {
      const target = await wiki(a.page);
      const conflict = await twoWriters(a, b, target);
      await expect
        .poll(() =>
          a.accessStreams.some(
            (stream) => stream.status === 200 && stream.contentType.startsWith("text/event-stream"),
          ),
        )
        .toBe(true);
      await b.page
        .getByTestId("off-body-conflict")
        .getByRole("button", { name: "최신 본문 직접 편집", exact: true })
        .click();
      expect((await editorShape(b.page)).text).toBe(extractText(conflict.current.contentJson));
      const ids = nodeIds(conflict.current.contentJson);
      const resolved = `${extractText(conflict.current.contentJson)} manually resolved`;
      const editor = b.page.locator(".fvoci-editor .ProseMirror");
      await editor.click();
      await b.page.keyboard.press("ControlOrMeta+End");
      await b.page.keyboard.type(" manually resolved");
      const saved = await save(b.page, target);
      expect(saved.command.expectedTailSeq).toBe(conflict.current.tailSeq);
      const current = await readBody(b.page, target);
      expect(extractText(current.contentJson)).toBe(resolved);
      expect(nodeIds(current.contentJson)).toEqual(ids);
      expectNative(current);
      expectForwardHistory(conflict.current, current);
      // Read-only oracle controls: matching current text does not excuse a
      // rewound native history, and native bytes must match retained formatting.
      expect(() => {
        expectForwardHistory(current, conflict.current);
      }).toThrow();
      const stripped = structuredClone(current);
      let removedMarks = 0;
      walkTiptap(stripped.contentJson, (node) => {
        if (Array.isArray(node.marks) && node.marks.length > 0) {
          delete node.marks;
          removedMarks++;
        }
      });
      expect(removedMarks).toBeGreaterThan(0);
      expect(() => {
        expectNative(stripped);
      }).toThrow();
      expect(await readBody(b.page, target)).toEqual(current);
      await revision(b.page, target, confirmed(saved.result), current);
      await openOff(fresh.page, target);
      expect(await readBody(fresh.page, target)).toEqual(current);
      expect((await editorShape(fresh.page)).text).toBe(resolved);
      const replay = await b.page.request.put(`${target.path}/body/versioned`, {
        data: saved.command,
      });
      expect(replay.status()).toBe(200);
      expect(await replay.json()).toEqual(saved.result);
      expect(await readBody(b.page, target)).toEqual(current);
      const wrong = await b.page.request.put(`${target.path}/body/versioned`, {
        data: { ...saved.command, expectedTailSeq: "999999" },
      });
      expect(wrong.status()).toBe(409);
      expect(await readBody(b.page, target)).toEqual(current);
      await b.page.reload();
      await expect(b.page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "true",
      );
      expect((await editorShape(b.page)).text).toBe(resolved);
      expect([...a.sockets, ...b.sockets, ...fresh.sockets]).toEqual([]);
    } finally {
      await Promise.all([a.context.close(), b.context.close(), fresh.context.close()]);
    }
  });

  test("wiki: lost real copy ACK retries one immutable command while a newer Markdown/IME draft stays owned", async ({
    browser,
  }) => {
    const a = await actor(browser, admin);
    const b = await actor(browser, member);
    const fresh = await actor(browser, member);
    try {
      const target = await wiki(a.page);
      const conflict = await twoWriters(a, b, target);
      const path = `/api/v1/workspaces/${workspaceId}/documents/from-draft`;
      const panel = b.page.getByTestId("off-body-conflict");
      await panel.getByLabel("문서 복제", { exact: true }).fill("private conflict copy");
      const sent: CopyCommand[] = [];
      let committed: CopyResult | undefined;
      await b.page.route(`**${path}`, async (route) => {
        sent.push(route.request().postDataJSON() as CopyCommand);
        const response = await route.fetch();
        expect(response.status()).toBe(201);
        committed = (await response.json()) as CopyResult;
        // Commit is real; only the browser's response delivery is lost.
        await route.abort("failed");
      });
      await panel.getByRole("button", { name: "복제", exact: true }).click();
      await expect.poll(() => sent.length).toBe(1);
      await expect.poll(() => committed?.document.id).toMatch(UUID_RE);
      await expect(panel.getByRole("button", { name: "복제", exact: true })).toBeEnabled();
      if (!committed) throw new Error("actual committed copy response missing");
      expect(committed.commandId).toBe(sent[0].commandId);
      expect(sent[0]).toMatchObject({
        sourceKind: "document",
        sourceId: target.id,
        title: "private conflict copy",
      });
      expect(extractText(sent[0].contentJson)).toBe(conflict.loser);
      await b.page.unroute(`**${path}`);
      await b.page.locator('[data-editor-mode="markdown"]').click();
      const buffer = b.page.getByRole("textbox", { name: "Markdown 직접 편집", exact: true });
      const later = `# unapplied newer draft ${randomUUID()}`;
      await buffer.fill(later);
      await buffer.dispatchEvent("compositionstart", { data: "한" });
      const retry = panel.getByRole("button", { name: "복제", exact: true });
      await expect(retry).toBeEnabled();
      let release: () => void = () => {};
      const gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      await b.page.route(`**${path}`, async (route) => {
        sent.push(route.request().postDataJSON() as CopyCommand);
        const response = await route.fetch();
        expect(response.status()).toBe(201);
        expect(await response.json()).toEqual(committed);
        await gate;
        await route.fulfill({ response });
      });
      try {
        await retry.click();
        await expect.poll(() => sent.length).toBe(2);
        expect(sent[1]).toEqual(sent[0]);
        await expect(
          panel.getByRole("button", { name: "복제하는 중…", exact: true }),
        ).toBeDisabled();
      } finally {
        release();
      }
      await expect(
        panel.getByRole("link", { name: "private conflict copy", exact: true }),
      ).toBeVisible();
      await expect(buffer).toHaveValue(later);
      await buffer.dispatchEvent("compositionend", { data: "한" });
      await expect(b.page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "false",
      );
      expect(await readBody(a.page, target)).toEqual(conflict.current);
      const displayId = committed.document.displayId;
      if (typeof displayId !== "string" || displayId.length === 0)
        throw new Error("committed copy displayId must be a nonempty string");
      const copy: Target = {
        id: committed.document.id,
        path: `/api/v1/workspaces/${workspaceId}/documents/${committed.document.id}`,
        url: `/w/${admin.workspaceSlug}/${displayId}`,
      };
      expect(copy.id).not.toBe(target.id);
      const copied = await readBody(fresh.page, copy);
      expect(copied.tailSeq).toBe(committed.tailSeq);
      expect(extractText(copied.contentJson)).toBe(conflict.loser);
      const originalMineIds = nodeIds(sent[0].contentJson);
      const copiedIds = nodeIds(copied.contentJson);
      expect(copiedIds.some((id) => originalMineIds.includes(id))).toBe(false);
      expectNative(copied);
      await revision(
        fresh.page,
        copy,
        {
          commandId: committed.commandId,
          targetId: copy.id,
          tailSeq: committed.tailSeq,
          revisionId: committed.revisionId,
        },
        copied,
      );
      await openOff(fresh.page, copy);
      expect((await editorShape(fresh.page)).text).toBe(conflict.loser);
      const wrong = await b.page.request.post(path, {
        data: { ...sent[0], title: "changed retry payload" },
      });
      expect(wrong.status()).toBe(409);
      expect(await readBody(a.page, target)).toEqual(conflict.current);
      expect([...a.sockets, ...b.sockets, ...fresh.sockets]).toEqual([]);
    } finally {
      await Promise.all([a.context.close(), b.context.close(), fresh.context.close()]);
    }
  });

  test("project document and document-origin task: same real OFF CAS and native revision contract", async ({
    browser,
  }) => {
    const a = await actor(browser, admin);
    const b = await actor(browser, member);
    const fresh = await actor(browser, member);
    try {
      const source = await wiki(a.page);
      await openOff(a.page, source);
      await replaceText(a.page, "actual origin block 中 😀");
      await save(a.page, source);
      const sourceBody = await readBody(a.page, source);
      const sourceBlock = nodeIds(sourceBody.contentJson)[0];
      const projects = `/api/v1/workspaces/${workspaceId}/projects`;
      const created = await a.page.request.post(projects, {
        data: {
          key: "OFF",
          name: "OFF selected project",
          visibility: "workspace",
        } satisfies Schema["CreateProjectBody"],
      });
      expect(created.status()).toBe(201);
      const project = (await created.json()) as Schema["ProjectOutput"];
      expect(project.rootDocumentId).toMatch(UUID_RE);
      const documents = `${projects}/${project.id}/documents`;
      const response = await a.page.request.post(documents, {
        data: {
          parentId: project.rootDocumentId,
          title: "OFF project body",
        } satisfies Schema["CreateProjectDocumentBody"],
      });
      expect(response.status()).toBe(201);
      const document = (await response.json()) as Schema["DocumentMetaResponse"];
      expect(document).toMatchObject({
        workspaceId,
        projectId: project.id,
        parentId: project.rootDocumentId,
      });
      if (!document.displayId) throw new Error("actual project document display ID missing");
      const taskCommand = {
        projectId: project.id,
        requestId: randomUUID(),
        anchor: sourceBlock,
        task: { title: "OFF origin task" },
      } satisfies Schema["DocumentTaskCreateBody"];
      const createdTask = await a.page.request.post(`${source.path}/tasks`, { data: taskCommand });
      expect(createdTask.status()).toBe(201);
      const { taskId } = (await createdTask.json()) as Schema["DocumentTaskCreateOutput"];
      const replay = await a.page.request.post(`${source.path}/tasks`, { data: taskCommand });
      expect(replay.status()).toBe(201);
      expect(await replay.json()).toEqual({ taskId });
      const taskRead = await a.page.request.get(
        `/api/v1/workspaces/${workspaceId}/tasks/${taskId}`,
      );
      expect(taskRead.status()).toBe(200);
      const task = (await taskRead.json()) as Schema["TaskOutput"];
      expect(task).toMatchObject({
        id: taskId,
        workspaceId,
        projectId: project.id,
        title: "OFF origin task",
      });
      const targets: Target[] = [
        {
          id: document.id,
          path: `${documents}/${document.id}`,
          url: `/w/${admin.workspaceSlug}/${document.displayId}`,
        },
        {
          id: taskId,
          path: `/api/v1/workspaces/${workspaceId}/tasks/${taskId}`,
          url: `/w/${admin.workspaceSlug}/${project.key}-${String(task.number)}`,
        },
      ];
      for (const target of targets) {
        const conflict = await twoWriters(a, b, target);
        await b.page
          .getByTestId("off-body-conflict")
          .getByRole("button", { name: "최신 본문 직접 편집", exact: true })
          .click();
        expect((await editorShape(b.page)).text).toBe(extractText(conflict.current.contentJson));
        await b.page.locator(".fvoci-editor .ProseMirror").click();
        await b.page.keyboard.press("ControlOrMeta+End");
        await b.page.keyboard.type(" selected manual resolution");
        const saved = await save(b.page, target);
        expect(saved.command.expectedTailSeq).toBe(conflict.current.tailSeq);
        const current = await readBody(b.page, target);
        expect(extractText(current.contentJson)).toBe(
          `${extractText(conflict.current.contentJson)} selected manual resolution`,
        );
        expect(nodeIds(current.contentJson)).toEqual(nodeIds(conflict.current.contentJson));
        expectNative(current);
        expectForwardHistory(conflict.current, current);
        await revision(b.page, target, confirmed(saved.result), current);
        await openOff(fresh.page, target);
        expect(await readBody(fresh.page, target)).toEqual(current);
        expect((await editorShape(fresh.page)).text).toBe(extractText(current.contentJson));
      }
      expect(await readBody(a.page, source)).toEqual(sourceBody);
      expect([...a.sockets, ...b.sockets, ...fresh.sockets]).toEqual([]);
    } finally {
      await Promise.all([a.context.close(), b.context.close(), fresh.context.close()]);
    }
  });

  test("personal note: actual capture UI, stable create, same-session multi-tab CAS and private fresh-client readback", async ({
    browser,
  }) => {
    const a = await actor(browser, admin);
    const fresh = await actor(browser, admin);
    const other = await actor(browser, member);
    try {
      await a.page.goto(`/w/${admin.workspaceSlug}/wiki`);
      await a.page.getByRole("button", { name: "개인 입력", exact: true }).click();
      const dialog = a.page.getByRole("dialog", { name: "개인 입력", exact: true });
      await dialog.getByRole("radio", { name: "메모", exact: true }).check();
      await dialog
        .getByRole("textbox", { name: "제목 또는 짧은 기록", exact: true })
        .fill("private actual OFF note");
      const sent = a.page.waitForRequest(
        (request) =>
          request.method() === "POST" &&
          new URL(request.url()).pathname.endsWith("/personal-input"),
      );
      const received = a.page.waitForResponse(
        (response) =>
          response.request().method() === "POST" &&
          new URL(response.url()).pathname.endsWith("/personal-input"),
      );
      await dialog.getByRole("button", { name: "개인 공간에 저장", exact: true }).click();
      const request = await sent;
      const command = request.postDataJSON() as Schema["PersonalInputBody"];
      expect(command.requestId).toMatch(UUID_RE);
      expect(command).toMatchObject({ intent: "note", title: "private actual OFF note" });
      const response = await received;
      expect(response.status()).toBe(201);
      const result = (await response.json()) as Schema["PersonalInputOutput"];
      expect(result.taskId).toBeNull();
      expect(result.projectId).toBeNull();
      const replay = await a.page.request.post(request.url(), { data: command });
      expect(replay.status()).toBe(201);
      expect(await replay.json()).toEqual({ ...result, replayed: true });
      await dialog.getByRole("button", { name: "메모 열기", exact: true }).click();
      await expect(a.page.locator('[data-body-mode="off"]')).toBeVisible();
      const url = new URL(a.page.url()).pathname;
      const personalId = new URL(request.url()).pathname.split("/")[4];
      if (!personalId) throw new Error("actual personal workspace route ID missing");
      expect(personalId).toMatch(UUID_RE);
      const target = {
        id: result.documentId,
        path: `/api/v1/workspaces/${personalId}/documents/${result.documentId}`,
        url,
      };
      // A new tab shares this exact session credential but owns a distinct draft.
      const secondPage = await a.context.newPage();
      const secondSockets: string[] = [];
      const secondAccessStreams: ActorPage["accessStreams"] = [];
      secondPage.on("response", (response) => {
        if (new URL(response.url()).pathname.endsWith("/access-stream"))
          secondAccessStreams.push({
            status: response.status(),
            contentType: response.headers()["content-type"] ?? "",
          });
      });
      secondPage.on("websocket", (socket) => secondSockets.push(socket.url()));
      const conflict = await twoWriters(
        a,
        {
          context: a.context,
          page: secondPage,
          sockets: secondSockets,
          accessStreams: secondAccessStreams,
        },
        target,
      );
      await secondPage
        .getByTestId("off-body-conflict")
        .getByRole("button", { name: "최신 본문 직접 편집", exact: true })
        .click();
      await secondPage.locator(".fvoci-editor .ProseMirror").click();
      await secondPage.keyboard.press("ControlOrMeta+End");
      await secondPage.keyboard.type(" private resolved");
      const saved = await save(secondPage, target);
      expect(saved.command.expectedTailSeq).toBe(conflict.current.tailSeq);
      const current = await readBody(secondPage, target);
      expect(extractText(current.contentJson)).toBe(
        `${extractText(conflict.current.contentJson)} private resolved`,
      );
      expectNative(current);
      expectForwardHistory(conflict.current, current);
      await revision(secondPage, target, confirmed(saved.result), current);
      await openOff(fresh.page, target);
      expect(await readBody(fresh.page, target)).toEqual(current);
      const denied = await other.page.request.get(`${target.path}/body/versioned`);
      expect(denied.status()).toBe(404);
      await secondPage.close();
      expect([...a.sockets, ...secondSockets, ...fresh.sockets, ...other.sockets]).toEqual([]);
    } finally {
      await Promise.all([a.context.close(), fresh.context.close(), other.context.close()]);
    }
  });

  test("wiki: lost real save response keeps the command through reload and rejects stale identity application", async ({
    browser,
  }) => {
    const a = await actor(browser, admin);
    const fresh = await actor(browser, member);
    try {
      const target = await wiki(a.page);
      await openOff(a.page, target);
      await replaceText(a.page, "unknown confirmed save 한글😀");
      const path = `${target.path}/body/versioned`;
      const commands: SaveCommand[] = [];
      let result: SaveResult | undefined;
      await a.page.route(`**${path}`, async (route) => {
        if (route.request().method() !== "PUT") return route.continue();
        commands.push(route.request().postDataJSON() as SaveCommand);
        const response = await route.fetch();
        expect(response.status()).toBe(200);
        result = (await response.json()) as SaveResult;
        await route.abort("failed");
      });
      await a.page.getByRole("button", { name: "저장", exact: true }).click();
      await expect.poll(() => result?.revisionId).toMatch(UUID_RE);
      await expect(a.page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "false",
      );
      await a.page.unroute(`**${path}`);
      await a.page.reload();
      await expect(a.page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "false",
      );
      expect((await editorShape(a.page)).text).toBe("unknown confirmed save 한글😀");
      const request = a.page.waitForRequest(
        (value) => value.method() === "PUT" && new URL(value.url()).pathname === path,
      );
      const response = a.page.waitForResponse(
        (value) => value.request().method() === "PUT" && new URL(value.url()).pathname === path,
      );
      await a.page.getByRole("button", { name: "저장", exact: true }).click();
      expect((await request).postDataJSON()).toEqual(commands[0]);
      expect((await response).status()).toBe(200);
      expect(await (await response).json()).toEqual(result);
      await expect(a.page.locator('[data-body-mode="off"]')).toHaveAttribute(
        "data-body-persisted",
        "true",
      );
      const current = await readBody(a.page, target);
      expect(current.tailSeq).toBe(result?.tailSeq);
      expectNative(current);
      await openOff(fresh.page, target);
      expect(await readBody(fresh.page, target)).toEqual(current);
      await replaceText(a.page, "private retired account draft");
      // Real login changes actor and credential in this tab. The old draft may
      // remain private in storage but cannot mount under the other actor.
      await login(a.page, member.email, member.password);
      await openOff(a.page, target);
      expect((await editorShape(a.page)).text).toBe(extractText(current.contentJson));
      expect((await editorShape(a.page)).text).not.toContain("private retired account draft");
      await replaceText(a.page, "healthy new actor progress");
      await save(a.page, target);
      expect(extractText((await readBody(a.page, target)).contentJson)).toBe(
        "healthy new actor progress",
      );
      expect([...a.sockets, ...fresh.sockets]).toEqual([]);
    } finally {
      await Promise.all([a.context.close(), fresh.context.close()]);
    }
  });

  test("wiki: delayed authorized read cannot overwrite the current A-B-A owner; unmounted request cancellation leaves healthy saves", async ({
    browser,
  }) => {
    const owner = await actor(browser, admin);
    const peer = await actor(browser, member);
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    try {
      const a = await wiki(owner.page);
      const b = await wiki(owner.page);
      await openOff(owner.page, a);
      await replaceText(owner.page, "old authorized A history");
      await save(owner.page, a);
      const old = await readBody(owner.page, a);
      await openOff(owner.page, b);
      await replaceText(owner.page, "separate B history");
      await save(owner.page, b);
      const bodyB = await readBody(owner.page, b);
      let settled: () => void = () => {};
      const settlement = new Promise<void>((resolve) => {
        settled = resolve;
      });
      let held = false;
      let delivered = false;
      const path = `${a.path}/body/versioned`;
      await peer.page.route(`**${path}`, async (route) => {
        if (held || route.request().method() !== "GET") return route.continue();
        held = true;
        const response = await route.fetch();
        expect(response.status()).toBe(200);
        expect(await response.json()).toEqual(old);
        delivered = true;
        await gate;
        // Page/lifetime cancellation is an actual transport outcome. A stale
        // response that remains deliverable still must not update the new owner.
        try {
          if (!route.request().failure()) await route.fulfill({ response });
        } finally {
          settled();
        }
      });
      await peer.page.goto(a.url);
      const lifetime = await peer.page.evaluate(() => performance.timeOrigin);
      await expect.poll(() => delivered).toBe(true);
      await openOff(owner.page, a);
      await replaceText(owner.page, "new current A history");
      await save(owner.page, a);
      const current = await readBody(owner.page, a);
      await wikiNavigate(peer.page, b);
      expect((await editorShape(peer.page)).text).toBe(extractText(bodyB.contentJson));
      await wikiNavigate(peer.page, a);
      expect((await editorShape(peer.page)).text).toBe(extractText(current.contentJson));
      expect(await peer.page.evaluate(() => performance.timeOrigin)).toBe(lifetime);
      release();
      await settlement;
      expect(await readBody(peer.page, a)).toEqual(current);
      expect((await editorShape(peer.page)).text).toBe("new current A history");
      await replaceText(peer.page, "healthy ABA save");
      const saved = await save(peer.page, a);
      expect(saved.command.expectedTailSeq).toBe(current.tailSeq);
      expect(extractText((await readBody(peer.page, a)).contentJson)).toBe("healthy ABA save");
      expect(await readBody(owner.page, b)).toEqual(bodyB);
      expect([...owner.sockets, ...peer.sockets]).toEqual([]);
    } finally {
      release();
      await Promise.all([owner.context.close(), peer.context.close()]);
    }
  });

  test("wiki: membership revoke denies the actual pending command without effects, owner still saves", async ({
    browser,
  }) => {
    const owner = await actor(browser, admin);
    const peer = await actor(browser, member);
    try {
      const target = await wiki(owner.page);
      await openOff(owner.page, target);
      await replaceText(owner.page, "retained authorized history");
      await save(owner.page, target);
      const before = await readBody(owner.page, target);
      await openOff(peer.page, target);
      await replaceText(peer.page, "must never commit after revoke");
      const path = `${target.path}/body/versioned`;
      let command: SaveCommand | undefined;
      let release: () => void = () => {};
      const gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      await peer.page.route(`**${path}`, async (route) => {
        if (route.request().method() !== "PUT") return route.continue();
        command = route.request().postDataJSON() as SaveCommand;
        await gate;
        // No fabricated refusal: the current server evaluates revoked rights.
        await route.continue();
      });
      const refused = peer.page.waitForResponse(
        (response) =>
          response.request().method() === "PUT" && new URL(response.url()).pathname === path,
      );
      await peer.page.getByRole("button", { name: "저장", exact: true }).click();
      await expect.poll(() => command?.commandId).toMatch(UUID_RE);
      try {
        const revoke = await owner.page.request.delete(
          `/api/v1/workspaces/${workspaceId}/members/${memberId}`,
        );
        expect(revoke.status()).toBe(200);
      } finally {
        release();
      }
      expect((await refused).status()).toBe(404);
      expect(await readBody(owner.page, target)).toEqual(before);
      const replay = await peer.page.request.put(path, { data: command });
      expect(replay.status()).toBe(404);
      expect(await readBody(owner.page, target)).toEqual(before);
      await expect(
        peer.page.locator('.fvoci-editor .ProseMirror[contenteditable="true"]'),
      ).toHaveCount(0);
      await replaceText(owner.page, "healthy owner after refusal");
      const saved = await save(owner.page, target);
      expect(saved.command.expectedTailSeq).toBe(before.tailSeq);
      const after = await readBody(owner.page, target);
      expect(extractText(after.contentJson)).toBe("healthy owner after refusal");
      expectNative(after);
      expect([...owner.sockets, ...peer.sockets]).toEqual([]);
    } finally {
      await Promise.all([owner.context.close(), peer.context.close()]);
    }
  });
});
