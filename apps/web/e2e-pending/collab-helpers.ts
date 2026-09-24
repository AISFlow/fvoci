import {
  devices,
  expect,
  test as base,
  type Browser,
  type BrowserContext,
  type Page,
} from "@playwright/test";
import { createE2eUser } from "../e2e/helpers";
import {
  attachmentNodesFromDocument,
  type AttachmentNodeShape,
} from "./collab-attachment-oracle.ts";
import { startOwnedServer, type OwnedServer } from "./collab-restart";
import {
  COLLAB_PERSIST_DONE,
  COLLAB_PERSIST_REQUEST,
  decodeHocuspocusFrame,
  frameBytes,
  persistParts,
  PROVIDER_VERSION,
  SESSION_COOKIE,
  UUID_RE,
  type CollabFrame,
} from "./collab-wire";

export { createE2eUser } from "../e2e/helpers";
export { expect };

export const test = base.extend<
  { baseURL: string; recycleCollab: void },
  { collabApp: OwnedServer }
>({
  collabApp: [
    async ({}, use) => {
      const server = await startOwnedServer();
      try {
        await use(server);
      } finally {
        await server.dispose();
      }
    },
    { scope: "worker" },
  ],
  recycleCollab: [
    async ({ collabApp }, use) => {
      await use();
      await collabApp.recycle();
    },
    { auto: true },
  ],
  baseURL: async ({ collabApp }, use) => {
    await use(collabApp.baseUrl);
  },
});

export async function newCollabContext(
  browser: Browser,
  baseUrl: string,
): Promise<BrowserContext> {
  return browser.newContext({
    ...devices["Desktop Chrome"],
    baseURL: baseUrl,
  });
}

export const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceName: "Acme 워크스페이스",
  workspaceSlug: "acme",
};

export const member = {
  email: "collab-member@example.com",
  password: "memberpass1",
  givenName: "협업",
  familyName: "멤버",
};

export const peer = {
  email: "collab-peer@example.com",
  password: "peerpass1",
  givenName: "동료",
  familyName: "편집",
};

export const MEMBER_PRESENCE = "멤버협업";
export const PEER_PRESENCE = "편집동료";

export type CollabWireLog = {
  sent: CollabFrame[];
  received: CollabFrame[];
};

export type BlockShape = {
  tag: string;
  id: string;
  text: string;
};

type EditorNode = {
  type: string;
  attrs?: Record<string, unknown>;
  text?: string;
  marks?: Array<{ type: string; attrs?: Record<string, unknown> }>;
  content?: EditorNode[];
};

export type EditorShape = {
  document: EditorNode;
  text: string;
  blocks: BlockShape[];
  bold: string[];
  italic: string[];
  hrefs: Array<{ href: string; text: string }>;
  table: { id: string; rows: string[][] } | null;
};

export type { AttachmentNodeShape };

export function attachmentNodes(shape: EditorShape): AttachmentNodeShape[] {
  return attachmentNodesFromDocument(shape.document);
}

export function attachmentNodeCount(shape: EditorShape): number {
  return attachmentNodes(shape).length;
}

export async function ensureCollabFixture(page: Page): Promise<void> {
  const setupRes = await page.request.get("/api/v1/setup");
  expect(setupRes.ok(), `setup status failed: ${setupRes.status()}`).toBe(true);
  const setup = (await setupRes.json()) as { needed: boolean };
  if (setup.needed) {
    await page.goto("/setup");
    await expect(page).toHaveURL(/\/setup$/);
    await fillInstanceSetup(page);
  }
  installCollabMember();
}

async function fillInstanceSetup(page: Page): Promise<void> {
  await page.getByLabel("성").fill(admin.familyName);
  await page.getByLabel("이름", { exact: true }).fill(admin.givenName);
  await page.getByLabel("이메일").fill(admin.email);
  await page.getByLabel("비밀번호").fill(admin.password);
  await page.getByLabel("워크스페이스 이름").fill(admin.workspaceName);
  await page.getByLabel("주소(영문)").fill(admin.workspaceSlug);
  await page.getByRole("button", { name: "시작하기" }).click();
  await expect(page).toHaveURL(/\/$/);
}

export async function ensureInstanceSetup(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/setup$/, { timeout: 15_000 });
  await fillInstanceSetup(page);
}

function isDuplicateEmailFixtureError(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const err = error as Error & { stderr?: Buffer | string; stdout?: Buffer | string };
  const text = [err.message, err.stderr?.toString(), err.stdout?.toString()].join("\n");
  return text.includes("users_email_unique");
}

function createE2eUserIfAbsent(
  email: string,
  password: string,
  givenName: string,
  options?: {
    familyName?: string;
    workspaceSlug?: string;
    membershipRole?: string;
  },
): void {
  try {
    createE2eUser(email, password, givenName, options);
  } catch (error) {
    if (!isDuplicateEmailFixtureError(error)) throw error;
  }
}

export async function login(page: Page, email: string, password: string): Promise<void> {
  await page.context().clearCookies();
  await page.goto("/login");
  await expect(page).toHaveURL(/\/login$/);
  await page.getByLabel("이메일").fill(email);
  await page.getByLabel("비밀번호").fill(password);
  await page.getByRole("button", { name: "로그인", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
}

export function installCollabMember(): void {
  createE2eUserIfAbsent(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
}

export function installCollabPeer(user = peer): void {
  createE2eUserIfAbsent(user.email, user.password, user.givenName, {
    familyName: user.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
}

export function attachCollabWire(page: Page): CollabWireLog {
  const log: CollabWireLog = { sent: [], received: [] };
  page.on("websocket", (ws) => {
    if (!ws.url().includes("/collab")) return;
    ws.on("framesent", (frame) => {
      const decoded = decodeHocuspocusFrame(frameBytes(frame.payload));
      if (decoded) log.sent.push(decoded);
    });
    ws.on("framereceived", (frame) => {
      const decoded = decodeHocuspocusFrame(frameBytes(frame.payload));
      if (decoded) log.received.push(decoded);
    });
  });
  return log;
}

export async function sessionCookie(context: BrowserContext): Promise<string> {
  const cookies = await context.cookies();
  const session = cookies.find((cookie) => cookie.name === SESSION_COOKIE);
  expect(session?.value).toBeTruthy();
  return session?.value ?? "";
}

export async function workspaceId(page: Page, slug: string): Promise<string> {
  const workspacesRes = await page.request.get("/api/v1/me/workspaces");
  expect(workspacesRes.ok()).toBe(true);
  const workspacesBody = await workspacesRes.json();
  const workspace = workspacesBody.items.find((item: { slug: string }) => item.slug === slug);
  expect(workspace).toBeTruthy();
  return workspace.id;
}

export type WikiDoc = {
  id: string;
  displayId: string;
  url: string;
  workspaceId: string;
};

export async function createWikiDoc(page: Page, title: string): Promise<WikiDoc> {
  const id = await workspaceId(page, admin.workspaceSlug);
  const res = await page.request.post(`/api/v1/workspaces/${id}/documents`, {
    data: { parentId: null, title },
  });
  expect(res.ok()).toBe(true);
  const body = await res.json();
  return {
    id: body.id,
    displayId: body.displayId,
    url: `/w/${admin.workspaceSlug}/${body.displayId}`,
    workspaceId: id,
  };
}

export async function waitConnected(page: Page): Promise<void> {
  const connected = page.locator('[data-collab-status="connected"]');
  try {
    await expect(connected).toBeVisible({ timeout: 15_000 });
  } catch (error) {
    const status = await page
      .locator("[data-collab-status]")
      .first()
      .getAttribute("data-collab-status")
      .catch(() => null);
    const collab = await page.locator(".document-page__collab").innerText().catch(() => "");
    throw new Error(
      `collab not connected url=${page.url()} status=${status} collab=${JSON.stringify(collab)} cause=${String(error)}`,
    );
  }
}

export async function waitDurableSaved(page: Page): Promise<void> {
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByText("연결됨 · 저장됨", { exact: true })).toBeVisible();
}

export async function expectNotDurablySaved(page: Page): Promise<void> {
  await expect(page.locator('[data-collab-persisted="true"]')).toHaveCount(0);
  await expect(page.getByText("연결됨 · 저장됨", { exact: true })).toHaveCount(0);
}

export function editorLocator(page: Page) {
  return page.locator(".fvoci-editor .ProseMirror");
}

export async function openEditor(page: Page, url: string) {
  const navigation = await page.goto(url);
  expect(navigation?.status(), "editor navigation must serve the React application").toBe(200);
  await waitConnected(page);
  const editor = editorLocator(page);
  await expect(editor).toBeVisible();
  return editor;
}

export async function clearEditor(page: Page): Promise<void> {
  const editor = editorLocator(page);
  await editor.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.press("Backspace");
}

export async function persistBody(page: Page): Promise<void> {
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await waitDurableSaved(page);
}

/** Structural oracle: exact text nodes, excluding awareness caret decorations. */
export async function editorShape(page: Page): Promise<EditorShape> {
  return editorLocator(page).evaluate((root) => {
    const isDecoration = (node: Node) => {
      const el = node instanceof Element ? node : node.parentElement;
      return Boolean(el?.closest(".collaboration-carets__caret, .collaboration-carets__label"));
    };
    const contentText = (from: Node) => {
      const walker = document.createTreeWalker(from, NodeFilter.SHOW_TEXT, {
        acceptNode(node) {
          return isDecoration(node) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
        },
      });
      let out = "";
      while (walker.nextNode()) out += walker.currentNode.nodeValue ?? "";
      return out;
    };
    const blocks = [...root.querySelectorAll("[data-id]")].map((el) => ({
      tag: el.tagName,
      id: el.getAttribute("data-id") ?? "",
      text: contentText(el),
    }));
    // Tiptap exposes the actual editor on its DOM root (Editor.ts). Read only:
    // resizable TableView deliberately omits node attrs such as id from its DOM.
    const editor = (root as HTMLElement & { editor?: { getJSON(): EditorNode } }).editor;
    if (!editor) throw new Error("missing live Tiptap editor for structural inspection");
    const documentNode = editor.getJSON();
    const tableNodes: EditorNode[] = [];
    const visit = (node: EditorNode) => {
      if (node.type === "table") tableNodes.push(node);
      for (const child of node.content ?? []) visit(child);
    };
    visit(documentNode);
    const tableEl = root.querySelector("table");
    if (Boolean(tableEl) !== (tableNodes.length > 0)) {
      throw new Error("rendered table and live document structure disagree");
    }
    const table = tableEl
      ? {
          id: typeof tableNodes[0]?.attrs?.id === "string" ? tableNodes[0].attrs.id : "",
          rows: [...tableEl.querySelectorAll("tr")].map((row) =>
            [...row.querySelectorAll("th, td")].map((cell) => contentText(cell)),
          ),
        }
      : null;
    return {
      document: documentNode,
      text: contentText(root),
      blocks,
      bold: [...root.querySelectorAll("strong")]
        .filter((el) => !isDecoration(el))
        .map((el) => contentText(el)),
      italic: [...root.querySelectorAll("em")]
        .filter((el) => !isDecoration(el))
        .map((el) => contentText(el)),
      hrefs: [...root.querySelectorAll("a[href]")]
        .filter((el) => !isDecoration(el))
        .map((el) => ({
          href: el.getAttribute("href") ?? "",
          text: contentText(el),
        })),
      table,
    };
  });
}

export type EditorSelectionSnapshot = {
  browser: string;
  editor: string | null;
  from: number | null;
  to: number | null;
};

export async function readEditorSelection(page: Page): Promise<EditorSelectionSnapshot> {
  return editorLocator(page).evaluate((root) => {
    const live = (root as HTMLElement & {
      editor?: {
        state: {
          selection: { from: number; to: number };
          doc: { textBetween(from: number, to: number): string };
        };
      };
    }).editor;
    const selection = live?.state.selection;
    return {
      browser: window.getSelection()?.toString() ?? "",
      editor: live && selection
        ? live.state.doc.textBetween(selection.from, selection.to)
        : null,
      from: selection?.from ?? null,
      to: selection?.to ?? null,
    };
  });
}

export async function closeCollabContext(
  context: BrowserContext,
  bodyFailed: boolean,
): Promise<void> {
  try {
    await context.close();
  } catch (error) {
    if (bodyFailed) return;
    throw error;
  }
}

export async function placeContentCaret(page: Page, where: "start" | "end"): Promise<void> {
  const locator = editorLocator(page);
  await locator.focus();
  const position = await locator.evaluate((root, edge) => {
    const isDecoration = (node: Node) => {
      const el = node instanceof Element ? node : node.parentElement;
      return Boolean(el?.closest(".collaboration-carets__caret, .collaboration-carets__label"));
    };
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode(node) {
        if (isDecoration(node)) return NodeFilter.FILTER_REJECT;
        if (!(node instanceof Text) || node.length === 0) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    let first: Text | null = null;
    let last: Text | null = null;
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (!first) first = node;
      last = node;
    }
    const target = edge === "start" ? first : last;
    if (!target) throw new Error("content caret requires an existing text node");
    const editor = (root as HTMLElement & {
      editor?: { view: { posAtDOM(node: Node, offset: number): number } };
    }).editor;
    if (!editor) throw new Error("missing live editor for caret inspection");
    const offset = edge === "start" ? 0 : target.length;
    const position = editor.view.posAtDOM(target, offset);
    const range = document.createRange();
    if (edge === "start") range.setStart(target, 0);
    else range.setStart(target, target.length);
    range.collapse(true);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
    return position;
  }, where);
  // Browser selectionchange is asynchronous. Observe the real ProseMirror
  // selection before typing; never dispatch a synthetic editor transaction.
  await expect.poll(() => locator.evaluate((root) => {
    const editor = (root as HTMLElement & {
      editor?: { state: { selection: { from: number; to: number } } };
    }).editor;
    return editor ? [editor.state.selection.from, editor.state.selection.to] : null;
  })).toEqual([position, position]);
}

export function uniqueBlockIds(shape: EditorShape): string[] {
  const ids: string[] = [];
  const visit = (node: EditorNode) => {
    if (node.attrs && "id" in node.attrs) {
      expect(node.attrs.id, `${node.type} must retain its UniqueID`).toEqual(expect.any(String));
      expect(node.attrs.id as string).toMatch(UUID_RE);
      ids.push(node.attrs.id as string);
    }
    for (const child of node.content ?? []) visit(child);
  };
  visit(shape.document);
  expect(ids.length, "live document must retain UniqueID blocks").toBeGreaterThan(0);
  if (shape.table) expect(shape.table.id).toMatch(UUID_RE);
  expect(new Set(ids).size).toBe(ids.length);
  return ids;
}

/** Display convenience only. Structural acceptance uses editorShape, not these. */
export async function expectTokens(page: Page, tokens: string[]): Promise<void> {
  const editor = editorLocator(page);
  for (const token of tokens) {
    await expect(editor).toContainText(token, { timeout: 15_000 });
  }
}

/** Display convenience only. Structural acceptance uses editorShape, not these. */
export async function expectTokensAbsent(page: Page, tokens: string[]): Promise<void> {
  const editor = editorLocator(page);
  for (const token of tokens) {
    await expect(editor).not.toContainText(token);
  }
}

export async function expectConverged(pageA: Page, pageB: Page): Promise<void> {
  await expect
    .poll(
      async () => {
        const left = await editorShape(pageA);
        const right = await editorShape(pageB);
        return JSON.stringify(left) === JSON.stringify(right) ? left : null;
      },
      { timeout: 15_000 },
    )
    .not.toBeNull();
  expect(await editorShape(pageA)).toEqual(await editorShape(pageB));
}

export async function insertSlashTable(page: Page): Promise<void> {
  const editor = editorLocator(page);
  await editor.click();
  await placeContentCaret(page, "end");
  await page.keyboard.press("Enter");
  await page.keyboard.type("/표");
  await page.keyboard.press("Enter");
  await expect(page.locator(".fvoci-editor table")).toBeVisible();
}

export async function applyBoldToSelection(page: Page): Promise<void> {
  await page.getByRole("button", { name: "굵게" }).click();
}

export async function applyLinkToSelection(page: Page, href: string): Promise<void> {
  await page.getByRole("button", { name: "링크" }).click();
  await page.getByLabel("URL").fill(href);
  await page.getByRole("button", { name: "적용" }).click();
}

export function sentPersistRequests(log: CollabWireLog): string[] {
  return log.sent.flatMap((frame) => {
    if (frame.kind !== "stateless") return [];
    const parts = persistParts(frame.payload);
    return parts?.kind === "request" ? [parts.id] : [];
  });
}

export function receivedPersistAcks(log: CollabWireLog): Array<{ kind: "done" | "failed"; id: string }> {
  return log.received.flatMap((frame) => {
    if (frame.kind !== "stateless") return [];
    const parts = persistParts(frame.payload);
    if (!parts || parts.kind === "request") return [];
    return [{ kind: parts.kind, id: parts.id }];
  });
}

export async function expectMatchingPersistAck(page: Page, log: CollabWireLog): Promise<string> {
  await expect.poll(() => sentPersistRequests(log).at(-1) ?? "").toMatch(UUID_RE);
  const requestId = sentPersistRequests(log).at(-1) ?? "";
  await expect
    .poll(() => receivedPersistAcks(log).find((ack) => ack.id === requestId) ?? null)
    .toEqual({ kind: "done", id: requestId });
  expect(
    receivedPersistAcks(log).some((ack) => ack.id === requestId && ack.kind === "failed"),
  ).toBe(false);
  expect(log.sent.some((frame) => frame.kind === "stateless" && frame.payload === `${COLLAB_PERSIST_REQUEST}:${requestId}`)).toBe(
    true,
  );
  expect(
    log.received.some(
      (frame) => frame.kind === "stateless" && frame.payload === `${COLLAB_PERSIST_DONE}:${requestId}`,
    ),
  ).toBe(true);
  await waitDurableSaved(page);
  return requestId;
}

export async function expectAwarenessTokenNotSession(
  log: CollabWireLog,
  session: string,
): Promise<void> {
  await expect
    .poll(() => log.sent.find((frame) => frame.kind === "auth-token") ?? null)
    .not.toBeNull();
  const auth = log.sent.find((frame) => frame.kind === "auth-token");
  if (!auth || auth.kind !== "auth-token") {
    throw new Error("missing auth-token frame");
  }
  expect(auth.token).toMatch(/^\d+$/);
  expect(auth.token).not.toBe(session);
  expect(auth.providerVersion).toBe(PROVIDER_VERSION);
  expect(auth.routingKey).toContain(":document:");
}

export async function indexedDbNames(page: Page): Promise<string[]> {
  return page.evaluate(async () => {
    if (!("databases" in indexedDB)) return [];
    const dbs = await indexedDB.databases();
    return dbs.map((db) => db.name ?? "");
  });
}

export type SlashAttachmentFixture =
  | string
  | { name: string; buffer: Buffer; mimeType?: string };

async function focusEditorForSlash(page: Page): Promise<void> {
  const editor = editorLocator(page);
  await editor.focus();
  await editor.evaluate((root) => {
    const live = (
      root as HTMLElement & {
        editor?: {
          chain(): {
            focus(): { setTextSelection(pos: number): { run(): boolean } };
          };
          state: { doc: { content: { size: number } } };
        };
      }
    ).editor;
    if (!live) throw new Error("editor instance missing on ProseMirror root");
    const size = live.state.doc.content.size;
    const pos = size > 0 ? Math.min(1, size) : 0;
    live.chain().focus().setTextSelection(pos).run();
  });
}

export async function insertSlashAttachment(
  page: Page,
  file: SlashAttachmentFixture,
): Promise<void> {
  await focusEditorForSlash(page);
  await page.keyboard.type("/첨부");
  await expect(page.locator(".fvoci-suggestion")).toBeVisible();
  await page.keyboard.press("Enter");
  const [fileChooser] = await Promise.all([
    page.waitForEvent("filechooser"),
    page.getByRole("button", { name: "파일 선택" }).click(),
  ]);
  if (typeof file === "string") {
    await fileChooser.setFiles(file);
  } else {
    await fileChooser.setFiles({
      name: file.name,
      mimeType: file.mimeType ?? "application/octet-stream",
      buffer: file.buffer,
    });
  }
  await expect(page.locator('.afn-attachment[data-state="stored"]')).toBeVisible({
    timeout: 30_000,
  });
}

export async function storedAttachmentDownloadBytes(page: Page): Promise<Buffer> {
  const href = await page.locator('.afn-attachment[data-state="stored"]').getAttribute("href");
  expect(href).toBeTruthy();
  const response = await page.request.get(href!);
  expect(response.ok()).toBe(true);
  return response.body();
}

export { UUID_RE };
