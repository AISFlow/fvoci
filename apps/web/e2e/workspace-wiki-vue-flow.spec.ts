// Wiki documents (/w/:slug/WIKI-<n>) are a page of the Vue app, with the
// collaborative editor bound to the real /collab room of the Rust server.
// Runs against the production build the Rust server serves, with the real
// PostgreSQL, Meilisearch and collab engine of the e2e group.
import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type BrowserContext,
  type Locator,
  type Page,
  type WebSocket as PlaywrightWebSocket,
} from "@playwright/test";
import type { EditorView } from "@tiptap/pm/view";
import { decodeHocuspocusFrame } from "../e2e-pending/collab-wire";
import { createE2eUser, login, watchCspViolations } from "./helpers";

test.describe.configure({ mode: "serial" });

const admin = {
  email: "Admin@Example.COM",
  password: "supersecret1",
  familyName: "김",
  givenName: "관리자",
  workspaceSlug: "vwiki",
  workspaceName: "Vue Wiki E2E",
};

const member = {
  email: "vue-wiki-member@example.com",
  password: "memberpass1",
  givenName: "편집",
  familyName: "동료",
};

// Icon sets must be bundled; these are the Iconify API hosts a runtime fetch would hit.
const ICON_API_HOSTS = ["api.iconify.design", "api.simplesvg.com", "api.unisvg.com"];

function watchIconRequests(page: Page): string[] {
  const hits: string[] = [];
  page.on("request", (request) => {
    const host = new URL(request.url()).hostname;
    if (ICON_API_HOSTS.includes(host)) hits.push(request.url());
  });
  return hits;
}

/** The page's /collab sockets that are open now. */
function watchCollabSockets(page: Page): { open: Set<PlaywrightWebSocket>; opened: () => number } {
  const open = new Set<PlaywrightWebSocket>();
  let opened = 0;
  page.on("websocket", (socket) => {
    if (!new URL(socket.url()).pathname.endsWith("/collab")) return;
    opened += 1;
    open.add(socket);
    socket.on("close", () => open.delete(socket));
  });
  return { open, opened: () => opened };
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
  } else if (
    page.url().includes("/login") ||
    (await page.getByRole("button", { name: "로그인", exact: true }).count()) > 0
  ) {
    await login(page, admin.email, admin.password);
  }
  // Setup starts at '/', then may cross Vue /login before returning home.
  // The URL alone can match before that redirect and interrupt a direct wiki goto.
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
}

async function workspaceId(request: APIRequestContext): Promise<string> {
  const res = await request.get("/api/v1/me/workspaces");
  expect(res.ok()).toBe(true);
  const workspace = (await res.json()).items.find(
    (item: { slug: string }) => item.slug === admin.workspaceSlug,
  );
  expect(workspace).toBeTruthy();
  return workspace.id;
}

type WikiDoc = { id: string; number: number; path: string };

async function createDoc(
  request: APIRequestContext,
  wsId: string,
  title: string,
  markdown?: string,
): Promise<WikiDoc> {
  const res = await request.post(`/api/v1/workspaces/${wsId}/documents`, {
    data: { parentId: null, title },
  });
  expect(res.status(), await res.text()).toBe(201);
  const doc = (await res.json()) as { id: string; number: number };
  if (markdown !== undefined) {
    const body = await request.put(`/api/v1/workspaces/${wsId}/documents/${doc.id}/body`, {
      data: { contentMd: markdown },
    });
    expect(body.ok(), await body.text()).toBe(true);
  }
  return { ...doc, path: `/w/${admin.workspaceSlug}/WIKI-${doc.number}` };
}

async function bodyJson(request: APIRequestContext, wsId: string, docId: string): Promise<string> {
  const res = await request.get(`/api/v1/workspaces/${wsId}/documents/${docId}/body`);
  expect(res.ok()).toBe(true);
  return JSON.stringify((await res.json()).contentJson);
}

function editorOf(page: Page): Locator {
  return page.locator(".fvoci-editor .ProseMirror");
}

async function openDoc(page: Page, path: string): Promise<Locator> {
  const navigation = await page.goto(path);
  expect(navigation?.status()).toBe(200);
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  const editor = editorOf(page);
  await expect(editor).toBeVisible();
  return editor;
}

/** Top-level block texts without peer caret labels (they are editor decorations). */
async function blockTexts(page: Page): Promise<string[]> {
  return editorOf(page).evaluate((root) =>
    [...root.children].map((block) => {
      const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT, {
        acceptNode: (node) =>
          node.parentElement?.closest(".collaboration-carets__caret, .collaboration-carets__label")
            ? NodeFilter.FILTER_REJECT
            : NodeFilter.FILTER_ACCEPT,
      });
      let text = "";
      while (walker.nextNode()) text += walker.currentNode.textContent ?? "";
      return text;
    }),
  );
}

async function expectBlocks(page: Page, expected: string[]): Promise<void> {
  await expect.poll(() => blockTexts(page), { timeout: 15_000 }).toEqual(expected);
}

/** Puts the caret at the end of top-level block `index` with a real click and End key. */
async function caretAtEndOf(page: Page, index: number): Promise<void> {
  await editorOf(page).locator(":scope > *").nth(index).click();
  await page.keyboard.press("End");
}

async function save(page: Page): Promise<void> {
  await page.getByRole("button", { name: "저장", exact: true }).click();
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });
}

async function newSignedInPage(browser: Browser, baseURL: string | undefined, who: { email: string; password: string }): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  await login(page, who.email, who.password);
  return { context, page };
}

test("direct URL and refresh serve the Vue page with the saved body", async ({ page }) => {
  await ensureSetup(page);
  createE2eUser(member.email, member.password, member.givenName, {
    familyName: member.familyName,
    workspaceSlug: admin.workspaceSlug,
    membershipRole: "member",
  });
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "Vue 직접 주소");

  const editor = await openDoc(page, doc.path);
  // The Vue app mounted #root (Vue marks its container), not the React app.
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await expect(page.getByTestId(`document-WIKI-${doc.number}`)).toBeVisible();
  await expect(page.getByLabel("문서 제목")).toHaveValue("Vue 직접 주소");
  await editor.click();
  await page.keyboard.type("새로 고쳐도 남는 본문");
  await save(page);
  expect(await bodyJson(page.request, wsId, doc.id)).toContain("새로 고쳐도 남는 본문");

  await page.reload();
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expectBlocks(page, ["새로 고쳐도 남는 본문"]);
  // One tab: no peer (a second provider of this tab would show as "나 (다른 탭)").
  await expect(page.locator(".document-page__presence")).toHaveCount(0);

  // A lower-case ref is the same document; the page stays on the Vue app.
  await openDoc(page, `/w/${admin.workspaceSlug}/wiki-${doc.number}`);
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);
  await expectBlocks(page, ["새로 고쳐도 남는 본문"]);

  // Leaving for a React page is a full page load.
  await page.getByRole("navigation", { name: "상위 경로" }).getByRole("link", { name: "위키", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}/wiki$`));
  await expect(page.getByRole("heading", { name: "위키" })).toBeVisible();
  await expect(page.locator("#root[data-v-app]")).toHaveCount(0);
  // ...and the React wiki list's link comes back to the Vue page.
  await page.getByRole("link", { name: "Vue 직접 주소" }).click();
  await expect(page).toHaveURL(new RegExp(`${doc.path}$`));
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#root[data-v-app]")).toHaveCount(1);

  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

type ClickCaret = { nativeBlock: number; parent: string; from: number; to: number };
type RemoteClickGate = {
  hold: boolean;
  pending: { socket: WebSocket; data: ArrayBuffer }[];
  before?: ClickCaret;
  after?: ClickCaret;
};

/** Delay real incoming /collab payloads, then deliver them in the native click
 * task, before selectionchange. This pins the caret/remote-update scheduling
 * boundary without a sleep, mocked document, or a replacement Yjs update. */
async function installRemoteClickGate(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const gate: RemoteClickGate = { hold: false, pending: [] };
    (window as unknown as { __fvociRemoteClickGate: RemoteClickGate }).__fvociRemoteClickGate = gate;
    const NativeSocket = window.WebSocket;
    window.WebSocket = class extends NativeSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        if (!new URL(url, location.href).pathname.endsWith("/collab")) return;
        // Registered before the provider's listeners. Redispatch preserves the
        // real server payload; only its delivery time is controlled by the test.
        this.addEventListener("message", (event: MessageEvent<ArrayBuffer>) => {
          if (!gate.hold) return;
          event.stopImmediatePropagation();
          if (gate.pending.length >= 32) throw new Error("remote click gate exceeded 32 frames");
          gate.pending.push({ socket: this, data: event.data });
        });
      }
    };
  });
}

test("undo and redo take back only this editor's own edits, on both peers", async ({ browser, baseURL }, testInfo) => {
  const a = await newSignedInPage(browser, baseURL, admin);
  const b = await newSignedInPage(browser, baseURL, member);
  const csp = watchCspViolations(a.page);
  try {
    const wsId = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, wsId, "되돌리기", "첫 문단\n\n둘째 문단\n");
    await installRemoteClickGate(b.page);
    await openDoc(a.page, doc.path);
    await openDoc(b.page, doc.path);
    await expectBlocks(a.page, ["첫 문단", "둘째 문단"]);
    await expectBlocks(b.page, ["첫 문단", "둘째 문단"]);
    await b.page.evaluate(() => {
      (window as unknown as { __fvociRemoteClickGate: RemoteClickGate }).__fvociRemoteClickGate.hold = true;
    });

    await caretAtEndOf(a.page, 0);
    await a.page.keyboard.type(" 에이");
    // Wait for an actual sync payload, not just a peer-awareness frame. Use
    // the existing bounded Hocuspocus observer, rather than a second codec.
    await expect.poll(async () => {
      const frames = await b.page.evaluate(() =>
        (window as unknown as { __fvociRemoteClickGate: RemoteClickGate }).__fvociRemoteClickGate.pending
          .map(({ data }) => [...new Uint8Array(data)]),
      );
      return frames.some((bytes) => {
        const frame = decodeHocuspocusFrame(new Uint8Array(bytes));
        return frame?.kind === "other" && frame.type === 0;
      });
    }).toBe(true);
    await b.page.evaluate(() => {
      const gate = (window as unknown as { __fvociRemoteClickGate: RemoteClickGate }).__fvociRemoteClickGate;
      const root = document.querySelector(".fvoci-editor .ProseMirror") as HTMLElement & { editor: { view: EditorView } };
      const snapshot = (): ClickCaret => {
        const native = document.getSelection();
        const selection = root.editor.view.state.selection;
        return {
          nativeBlock: [...root.children].findIndex((block) => block.contains(native?.anchorNode ?? null)),
          parent: selection.$from.parent.textContent,
          from: selection.from,
          to: selection.to,
        };
      };
      document.addEventListener("click", () => {
        gate.before = snapshot();
        gate.hold = false;
        for (const { socket, data } of gate.pending.splice(0)) {
          socket.dispatchEvent(new MessageEvent("message", { data }));
        }
        gate.after = snapshot();
      }, { capture: true, once: true });
    });
    await caretAtEndOf(b.page, 1);
    const boundary = await b.page.evaluate(() => {
      const { before, after } = (window as unknown as { __fvociRemoteClickGate: RemoteClickGate }).__fvociRemoteClickGate;
      return { before, after };
    });
    await testInfo.attach("native-click-remote-boundary", {
      body: JSON.stringify(boundary), contentType: "application/json",
    });
    expect(boundary.before).toMatchObject({ nativeBlock: 1, parent: "둘째 문단" });
    expect(boundary.after).toMatchObject({ nativeBlock: 1, parent: "둘째 문단" });
    expect(boundary.before?.from).toBe(boundary.before?.to);
    expect(boundary.after?.from).toBe(boundary.after?.to);
    await b.page.keyboard.type(" 비");
    await expectBlocks(a.page, ["첫 문단 에이", "둘째 문단 비"]);
    await expectBlocks(b.page, ["첫 문단 에이", "둘째 문단 비"]);

    await a.page.keyboard.press("Control+z");
    await expectBlocks(a.page, ["첫 문단", "둘째 문단 비"]);
    await expectBlocks(b.page, ["첫 문단", "둘째 문단 비"]);

    await a.page.keyboard.press("Control+Shift+z");
    await expectBlocks(a.page, ["첫 문단 에이", "둘째 문단 비"]);
    await expectBlocks(b.page, ["첫 문단 에이", "둘째 문단 비"]);

    // B's own undo takes back B's text and leaves A's.
    await b.page.keyboard.press("Control+z");
    await expectBlocks(a.page, ["첫 문단 에이", "둘째 문단"]);
    await expectBlocks(b.page, ["첫 문단 에이", "둘째 문단"]);

    // The toolbar's undo is the same command.
    await a.page.getByRole("button", { name: "실행 취소" }).click();
    await expectBlocks(b.page, ["첫 문단", "둘째 문단"]);
    await a.page.getByRole("button", { name: "다시 실행" }).click();
    await expectBlocks(b.page, ["첫 문단 에이", "둘째 문단"]);

    await save(a.page);
    expect(await bodyJson(a.page.request, wsId, doc.id)).toContain("첫 문단 에이");
    expect(csp).toEqual([]);
  } finally {
    await a.context.close();
    await b.context.close();
  }
});

test("a math block inserted with /math reaches the peer, the saved body and a reload", async ({
  browser,
  baseURL,
}) => {
  const a = await newSignedInPage(browser, baseURL, admin);
  const b = await newSignedInPage(browser, baseURL, member);
  const csp = watchCspViolations(a.page);
  const cspPeer = watchCspViolations(b.page);
  try {
    const wsId = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, wsId, "수식");
    const editor = await openDoc(a.page, doc.path);
    await openDoc(b.page, doc.path);

    await editor.click();
    await a.page.keyboard.type("/math");
    await expect(a.page.locator(".fvoci-suggestion")).toBeVisible();
    await a.page.keyboard.press("Enter");
    const empty = a.page.getByRole("button", { name: "$$ (비어 있음)" });
    await expect(empty).toBeVisible();
    await expect(b.page.locator(".fvoci-editor .afn-math")).toContainText("$$ (비어 있음)");

    // An unfinished source renders KaTeX's error span, whose style= is stripped
    // without an in-page parse (no CSP report), then the finished one.
    await empty.click();
    const source = a.page.getByLabel("수식 LaTeX");
    await expect(source).toBeFocused();
    await source.fill("\\frac{");
    await source.blur();
    await expect(a.page.locator(".fvoci-editor .afn-math .katex-error")).toBeVisible();
    await a.page.locator(".fvoci-editor .afn-math").click();
    await a.page.getByLabel("수식 LaTeX").fill("\\frac{a}{b}");
    await a.page.getByLabel("수식 LaTeX").blur();

    await expect(a.page.locator(".fvoci-editor .afn-math math mfrac")).toBeVisible();
    // The peer's editor renders the attribute it received through Yjs as MathML.
    await expect(b.page.locator(".fvoci-editor .afn-math math mfrac")).toBeVisible({ timeout: 15_000 });
    await expect(b.page.locator(".fvoci-editor .afn-math annotation")).toHaveText("\\frac{a}{b}");

    await save(a.page);
    const saved = await bodyJson(a.page.request, wsId, doc.id);
    expect(saved).toContain('"type":"math"');
    expect(saved).toContain("\\\\frac{a}{b}");

    await a.page.reload();
    await expect(a.page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
    await expect(a.page.locator(".fvoci-editor .afn-math math mfrac")).toBeVisible();
    expect(csp).toEqual([]);
    expect(cspPeer).toEqual([]);
  } finally {
    await a.context.close();
    await b.context.close();
  }
});

test("a peer's change to an embed or math block being edited keeps the typed draft, and leaving the field commits it", async ({
  browser,
  baseURL,
}) => {
  // Not inline math: a peer's change to an inline node rebuilds its
  // paragraph and the node view with it, in the React editor as well, so
  // the open field closes there (a separate, pre-existing issue: #259).
  const a = await newSignedInPage(browser, baseURL, admin);
  const b = await newSignedInPage(browser, baseURL, member);
  try {
    const wsId = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, wsId, "편집 중 원격 변경");
    const paragraph = (text: string) => ({ type: "paragraph", content: [{ type: "text", text }] });
    const put = await a.page.request.put(`/api/v1/workspaces/${wsId}/documents/${doc.id}/body`, {
      data: {
        contentJson: {
          type: "doc",
          content: [
            paragraph("앞"),
            { type: "embed", attrs: { entity: "document", ref: "WIKI-9999" } },
            { type: "math", attrs: { latex: "x" } },
            paragraph("뒤"),
          ],
        },
      },
    });
    expect(put.ok(), await put.text()).toBe(true);
    await openDoc(a.page, doc.path);
    await openDoc(b.page, doc.path);

    // Embed: A types a new reference; meanwhile B changes the embed's kind.
    await a.page.getByRole("button", { name: "참조 편집" }).click();
    const refA = a.page.getByLabel("참조", { exact: true });
    await expect(refA).toBeFocused();
    await refA.fill("WIKI-1234");
    await b.page.getByRole("button", { name: "참조 편집" }).click();
    await b.page.getByLabel("참조 종류").selectOption("task");
    await caretAtEndOf(b.page, 3);
    // A's form follows the node (its kind attribute is now B's)...
    await expect(a.page.locator(".fvoci-editor .afn-embed-edit")).toHaveAttribute("data-entity", "task");
    // ...and keeps what A typed and chose, as the React view's uncontrolled fields do.
    await expect(refA).toBeFocused();
    await expect(refA).toHaveValue("WIKI-1234");
    await expect(a.page.getByLabel("참조 종류")).toHaveValue("document");
    // Leaving the form commits A's form: the last writer wins, on both peers.
    await caretAtEndOf(a.page, 0);
    for (const page of [a.page, b.page]) {
      const card = page.locator(".fvoci-editor .afn-embed-host .afn-embed");
      await expect(card).toHaveAttribute("data-entity", "document");
      await expect(card.locator(".afn-embed-ref")).toHaveText("WIKI-1234");
    }

    // Block math: A types a new source while B commits another one.
    await a.page.locator(".fvoci-editor button.afn-math").click();
    const sourceA = a.page.getByLabel("수식 LaTeX");
    await expect(sourceA).toBeFocused();
    await sourceA.fill("a+b");
    await b.page.locator(".fvoci-editor button.afn-math").click();
    await b.page.getByLabel("수식 LaTeX").fill("y");
    await b.page.getByLabel("수식 LaTeX").blur();
    await expect(b.page.locator(".fvoci-editor .afn-math annotation")).toHaveText("y");
    // B's later text edit reaches A after B's latex (one socket, in order).
    await caretAtEndOf(b.page, 3);
    await b.page.keyboard.type(" 끝");
    await expect.poll(async () => (await blockTexts(a.page)).at(-1), { timeout: 15_000 }).toBe("뒤 끝");
    await expect(sourceA).toBeFocused();
    await expect(sourceA).toHaveValue("a+b");
    await sourceA.blur();
    await expect(a.page.locator(".fvoci-editor .afn-math annotation")).toHaveText("a+b");
    await expect(b.page.locator(".fvoci-editor .afn-math annotation")).toHaveText("a+b", { timeout: 15_000 });

    await save(a.page);
    const saved = JSON.parse(await bodyJson(a.page.request, wsId, doc.id)) as {
      content: { type: string; attrs?: Record<string, unknown> }[];
    };
    expect(saved.content.find((node) => node.type === "embed")?.attrs).toMatchObject({
      entity: "document",
      ref: "WIKI-1234",
    });
    expect(saved.content.find((node) => node.type === "math")?.attrs).toMatchObject({ latex: "a+b" });
  } finally {
    await a.context.close();
    await b.context.close();
  }
});

test("Korean composition survives a concurrent remote edit, then undoes and redoes", async ({
  browser,
  baseURL,
}) => {
  const a = await newSignedInPage(browser, baseURL, admin);
  const b = await newSignedInPage(browser, baseURL, member);
  try {
    const wsId = await workspaceId(a.page.request);
    const doc = await createDoc(a.page.request, wsId, "한글 조합", "첫 문단\n\n둘째 문단\n");
    await openDoc(a.page, doc.path);
    await openDoc(b.page, doc.path);
    await caretAtEndOf(a.page, 0);

    // Chromium's IME input path over CDP: composition events with marked
    // text, each step replacing the syllable being composed, then a commit.
    // The composing syllable is the selected block, as Korean IMEs show it.
    // Synthetic events: this is not the OS IME witness. With the caret at
    // the end of the marked text previously left the first jamo behind.
    // The next test pins the repaired path; the IBus witness recorded the bug.
    const ime = await a.context.newCDPSession(a.page);
    const setComposition = (text: string) =>
      ime.send("Input.imeSetComposition", { text, selectionStart: 0, selectionEnd: text.length });
    const compose = async (steps: string[], commit: string) => {
      for (const text of steps) await setComposition(text);
      await ime.send("Input.insertText", { text: commit });
    };

    await setComposition("ㅎ");
    // A remote edit in another paragraph lands while A is composing.
    await caretAtEndOf(b.page, 1);
    await b.page.keyboard.type(" 원격");
    await expectBlocks(a.page, ["첫 문단ㅎ", "둘째 문단 원격"]);
    await compose(["하", "한"], "한");
    await compose(["ㄱ", "그", "글"], "글");

    await expectBlocks(a.page, ["첫 문단한글", "둘째 문단 원격"]);
    await expectBlocks(b.page, ["첫 문단한글", "둘째 문단 원격"]);

    // Undo takes back A's composed text and never B's. The Yjs undo manager
    // groups edits by time (500 ms), so the pause while the remote edit
    // arrived mid-composition split A's text into more than one step.
    let undos = 0;
    while ((await blockTexts(a.page))[0] !== "첫 문단") {
      expect(undos, "A's composed text is undone within three steps").toBeLessThan(3);
      const before = (await blockTexts(a.page))[0];
      await a.page.keyboard.press("Control+z");
      undos += 1;
      await expect.poll(async () => (await blockTexts(a.page))[0]).not.toBe(before);
      expect((await blockTexts(a.page))[1]).toBe("둘째 문단 원격");
    }
    await expectBlocks(b.page, ["첫 문단", "둘째 문단 원격"]);
    for (let i = 0; i < undos; i += 1) await a.page.keyboard.press("Control+Shift+z");
    await expectBlocks(a.page, ["첫 문단한글", "둘째 문단 원격"]);
    await expectBlocks(b.page, ["첫 문단한글", "둘째 문단 원격"]);

    await save(a.page);
    expect(await bodyJson(a.page.request, wsId, doc.id)).toContain("첫 문단한글");
  } finally {
    await a.context.close();
    await b.context.close();
  }
});

// #268 (`83c01480`) skips UniqueID setNodeMarkup while a transaction has
// composition meta, so Chromium/IBus no longer restarts the first Hangul
// step. #258 is closed. This used to be test.fail; CI on main f3f53c90
// (Web 36599369890 shard 5) failed with "Expected to fail, but passed".
// Keep the assertion as a regression: "첫 문단" + 한글 must be "첫 문단한글",
// not "첫 문단ㅎ한글".
test("Korean composition with the caret after the marked text leaves no stray jamo", async ({ page }) => {
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "한글 조합 캐럿", "첫 문단\n");
  await openDoc(page, doc.path);
  await caretAtEndOf(page, 0);
  const ime = await page.context().newCDPSession(page);
  const compose = async (steps: string[], commit: string) => {
    for (const text of steps) {
      await ime.send("Input.imeSetComposition", { text, selectionStart: text.length, selectionEnd: text.length });
    }
    await ime.send("Input.insertText", { text: commit });
  };
  await compose(["ㅎ", "하", "한"], "한");
  await compose(["ㄱ", "그", "글"], "글");
  await expect.poll(() => blockTexts(page), { timeout: 5_000 }).toEqual(["첫 문단한글"]);
});

test("moving between five documents in the app keeps one room socket and every edit", async ({ page }) => {
  await login(page, admin.email, admin.password);
  const csp = watchCspViolations(page);
  const sockets = watchCollabSockets(page);
  const wsId = await workspaceId(page.request);
  // WIKI numbers ascend with creation; each later document becomes the parent
  // of the one before it, so every page links to the next through its breadcrumb.
  const docs: WikiDoc[] = [];
  for (let i = 1; i <= 5; i += 1) docs.push(await createDoc(page.request, wsId, `이동 ${i}`));
  for (let i = 0; i < 4; i += 1) {
    const moved = await page.request.post(`/api/v1/workspaces/${wsId}/documents/${docs[i]!.id}/move`, {
      data: { newParentId: docs[i + 1]!.id },
    });
    expect(moved.ok(), await moved.text()).toBe(true);
  }

  const expectRoom = async (doc: WikiDoc) => {
    await expect(page).toHaveURL(new RegExp(`${doc.path}$`));
    await expect(page.getByTestId(`document-WIKI-${doc.number}`)).toBeVisible();
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
    await expect(editorOf(page)).toBeVisible();
    // The previous room's socket is flushed and closed; only this room's is open.
    await expect.poll(() => sockets.open.size, { timeout: 5_000 }).toBe(1);
  };

  await openDoc(page, docs[0]!.path);
  await expectRoom(docs[0]!);
  // When the last keystroke and the link click reached the page.
  await page.evaluate(() => {
    const marks = window as unknown as { lastKeyAt: number; lastLinkAt: number };
    addEventListener("keydown", () => (marks.lastKeyAt = performance.now()), true);
    addEventListener("click", (event) => {
      if ((event.target as Element | null)?.closest("a")) marks.lastLinkAt = performance.now();
    }, true);
  });
  const gaps: number[] = [];
  for (let i = 0; i < 4; i += 1) {
    const next = docs[i + 1]!;
    await editorOf(page).click();
    await page.keyboard.type(`떠나기 직전 ${i + 1}`);
    // An in-app move (a router link) inside the editor's 200 ms update batch.
    await page
      .getByRole("navigation", { name: "상위 경로" })
      .getByRole("link", { name: `이동 ${i + 2}`, exact: true })
      .click();
    gaps.push(
      await page.evaluate(() => {
        const marks = window as unknown as { lastKeyAt: number; lastLinkAt: number };
        return marks.lastLinkAt - marks.lastKeyAt;
      }),
    );
    await expectRoom(next);
  }
  // The fifth room connected: no socket leaked against the per-session cap.
  expect(sockets.opened()).toBe(5);
  for (const gap of gaps) expect(gap).toBeLessThan(200);

  for (let i = 3; i >= 0; i -= 1) {
    await page.goBack();
    await expectRoom(docs[i]!);
    await expectBlocks(page, [`떠나기 직전 ${i + 1}`]);
  }
  expect(await bodyJson(page.request, wsId, docs[0]!.id)).toContain("떠나기 직전 1");
  expect(csp).toEqual([]);
});
