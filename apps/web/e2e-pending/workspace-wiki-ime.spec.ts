/**
 * #258: Korean IME composition in blocks that have no UniqueID yet, on the
 * wiki page. Blocks of a body the server seeded (body PUT, imports) and
 * a new document's first paragraph have no id; the editor used to give the
 * block its id in the same dispatch as the composition's first update, which
 * redrew the block under the composition, and Chromium then kept the first
 * jamo on its own ("첫 문단" + 한글 gave "첫 문단ㅎ한글"). The fix is the
 * UniqueID filter in packages/editor/src/tiptap-schema.ts.
 *
 * 1. CDP composition (runs in the collaboration suite): Input.imeSetComposition
 *    with the caret at the end of the marked text, as Chromium reports it for
 *    IBus Hangul, at the end of a paragraph, in an empty paragraph and inside
 *    a word, without and with a peer's edit during the preedit, and as a new
 *    document's first input. Synthetic events on Chromium's IME path; these
 *    fail without the fix.
 * 2. The OS IME witness (skipped unless FVOCI_E2E_OS_IME=1): the same cases
 *    with real XTEST keys through IBus Hangul (2-set) into a headed Chromium,
 *    a control in a plain contenteditable, and a flow with preedit Backspace,
 *    undo/redo, Save and a server restart. Needs, on Linux, Xvfb, xdotool,
 *    dbus-run-session and ibus with the hangul engine. One way to run it (a
 *    private display, D-Bus session and IBus configuration):
 *
 *      export DISPLAY=:233; Xvfb :233 -screen 0 1280x1024x24 -nolisten tcp &
 *      HOME=$(mktemp -d) dbus-run-session -- bash -c '
 *        gsettings set org.freedesktop.ibus.engine.hangul initial-input-mode hangul
 *        ibus-daemon --daemonize --replace --xim --panel=disable; sleep 2
 *        ibus engine hangul
 *        export IBUS_ADDRESS=$(ibus address) GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
 *        export HOME=<your home> FVOCI_E2E_PENDING=1 FVOCI_E2E_OS_IME=1
 *        bash scripts/web-e2e-run-group.sh e2e-pending/workspace-wiki-ime.spec.ts
 *        ibus exit'
 */
import { execFileSync } from "node:child_process";
import { chromium, type Browser, type BrowserContext, type Page } from "@playwright/test";
import {
  createWikiDoc,
  editorLocator,
  ensureCollabFixture,
  expect,
  installCollabPeer,
  login,
  member,
  newCollabContext,
  peer,
  test,
  UUID_RE,
  waitConnected,
  type WikiDoc,
} from "./collab-helpers";

/** This spec's own peer: workspace-wiki-collab.spec.ts revokes the shared one. */
const imePeer = { ...peer, email: "collab-ime-peer@example.com" };

/** The seeded body: blocks without ids, as the server stores a PUT body. */
const SEED = ["첫 문단", "둘째 문단", ""];

type Where = "end" | "empty" | "mid";
/** Where each case composes: top-level block and character offset. */
const CARET: Record<Where, { block: number; offset: number }> = {
  end: { block: 0, offset: SEED[0].length },
  empty: { block: 2, offset: 0 },
  mid: { block: 1, offset: 1 },
};

/** The seed with `typed` inserted at the case's caret. */
function seedWith(where: Where, typed: string, texts = SEED): string[] {
  const { block, offset } = CARET[where];
  return texts.map((text, i) => (i === block ? text.slice(0, offset) + typed + text.slice(offset) : text));
}

/** The block a peer edits meanwhile: never the one being composed in. */
const peerBlock = (where: Where) => (CARET[where].block === 1 ? 0 : 1);

async function seededDoc(page: Page, title: string): Promise<WikiDoc> {
  const doc = await createWikiDoc(page, title);
  const paragraph = (text: string) => ({
    type: "paragraph",
    ...(text ? { content: [{ type: "text", text }] } : {}),
  });
  const put = await page.request.put(`/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`, {
    data: { contentJson: { type: "doc", content: SEED.map(paragraph) } },
  });
  expect(put.ok(), await put.text()).toBe(true);
  return doc;
}

/** Top-level block texts in the DOM, without peer caret labels. */
async function blockTexts(page: Page): Promise<string[]> {
  return editorLocator(page).evaluate((root) =>
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

type LiveEditor = {
  state: {
    doc: {
      forEach(fn: (node: { textContent: string; attrs: Record<string, unknown> }) => void): void;
    };
    selection: { empty: boolean; $from: { index(depth: number): number; parentOffset: number } };
  };
};

/** The editor model's top-level blocks: text and UniqueID. */
async function modelBlocks(page: Page): Promise<Array<{ text: string; id: unknown }>> {
  return editorLocator(page).evaluate((root) => {
    const editor = (root as HTMLElement & { editor?: LiveEditor }).editor;
    if (!editor) throw new Error("missing live editor");
    const out: Array<{ text: string; id: unknown }> = [];
    editor.state.doc.forEach((node) => out.push({ text: node.textContent, id: node.attrs.id }));
    return out;
  });
}

/** The editor selection as top-level block and offset, null unless collapsed. */
async function caret(page: Page): Promise<{ block: number; offset: number } | null> {
  return editorLocator(page).evaluate((root) => {
    const editor = (root as HTMLElement & { editor?: LiveEditor }).editor;
    const selection = editor?.state.selection;
    if (!selection?.empty) return null;
    return { block: selection.$from.index(0), offset: selection.$from.parentOffset };
  });
}

/** Model and DOM agree on the block texts, which equal `expected`. */
async function expectBlocks(page: Page, expected: string[]): Promise<void> {
  await expect.poll(() => blockTexts(page), { timeout: 15_000 }).toEqual(expected);
  expect((await modelBlocks(page)).map((block) => block.text)).toEqual(expected);
}

/** Puts the caret with a click and the keyboard, as a user does. */
async function placeCaret(page: Page, where: Where): Promise<void> {
  const { block, offset } = CARET[where];
  await editorLocator(page).locator(":scope > *").nth(block).click();
  if (where === "mid") {
    await page.keyboard.press("Home");
    for (let i = 0; i < offset; i += 1) await page.keyboard.press("ArrowRight");
  } else {
    await page.keyboard.press("End");
  }
  await expect.poll(() => caret(page)).toEqual({ block, offset });
}

/** A peer appends " 원격" to a block and A receives it. */
async function peerEdit(a: Page, b: Page, block: number, expected: string[]): Promise<void> {
  await editorLocator(b).locator(":scope > *").nth(block).click();
  await b.keyboard.press("End");
  await b.keyboard.type(" 원격");
  await expect.poll(() => blockTexts(a), { timeout: 15_000 }).toEqual(expected);
}

const withRemote = (texts: string[], block: number) =>
  texts.map((text, i) => (i === block ? `${text} 원격` : text));

for (const where of ["end", "empty", "mid"] as const) {
  for (const remote of [false, true]) {
    test(`CDP composition with the caret after the marked text (${where}${remote ? ", peer edit during preedit" : ""}) leaves no stray jamo`, async ({
      browser,
      collabApp,
    }) => {
      const ctxA = await newCollabContext(browser, collabApp.baseUrl);
      const ctxB = await newCollabContext(browser, collabApp.baseUrl);
      try {
        const a = await ctxA.newPage();
        const b = await ctxB.newPage();
        await ensureCollabFixture(a);
        installCollabPeer(imePeer);
        await login(a, member.email, member.password);
        const doc = await seededDoc(a, `IME 조합 ${where}${remote ? " 원격" : ""}`);
        await a.goto(doc.url);
        await waitConnected(a);
        if (remote) {
          await login(b, imePeer.email, imePeer.password);
          await b.goto(doc.url);
          await waitConnected(b);
        }
        await expectBlocks(a, SEED);
        expect((await modelBlocks(a)).map((block) => block.id)).toEqual([null, null, null]);
        await placeCaret(a, where);

        // Chromium's IME input path: each step replaces the marked text, the
        // caret after it (selectionStart = selectionEnd = end), as Chromium
        // reports it for IBus Hangul; the commit ends each syllable.
        const ime = await ctxA.newCDPSession(a);
        const mark = (text: string) =>
          ime.send("Input.imeSetComposition", {
            text,
            selectionStart: text.length,
            selectionEnd: text.length,
          });
        const commit = (text: string) => ime.send("Input.insertText", { text });

        await mark("ㅎ");
        await expect.poll(() => blockTexts(a)).toEqual(seedWith(where, "ㅎ"));
        let texts = SEED;
        if (remote) {
          texts = withRemote(SEED, peerBlock(where));
          await peerEdit(a, b, peerBlock(where), seedWith(where, "ㅎ", texts));
        }
        await mark("하");
        await mark("한");
        await commit("한");
        for (const text of ["ㄱ", "그", "글"]) await mark(text);
        await commit("글");
        await expectBlocks(a, seedWith(where, "한글", texts));
        if (remote) await expectBlocks(b, seedWith(where, "한글", texts));

        // UniqueID still gives the block an id, at the next plain edit.
        await a.keyboard.press("Space");
        await expectBlocks(a, seedWith(where, "한글 ", texts));
        const ids = (await modelBlocks(a)).map((block) => block.id);
        expect(ids[CARET[where].block]).toEqual(expect.stringMatching(UUID_RE));
        const assigned = ids.filter((id) => id !== null);
        expect(new Set(assigned).size).toBe(assigned.length);
      } finally {
        await ctxA.close();
        await ctxB.close();
      }
    });
  }
}

test("CDP composition as the first input of a new document leaves no stray jamo", async ({ browser, collabApp }) => {
  const ctx = await newCollabContext(browser, collabApp.baseUrl);
  try {
    const a = await ctx.newPage();
    await ensureCollabFixture(a);
    await login(a, member.email, member.password);
    const doc = await createWikiDoc(a, "IME 새 문서");
    await a.goto(doc.url);
    await waitConnected(a);
    // A new document shows one empty paragraph that has no id yet.
    await expectBlocks(a, [""]);
    expect((await modelBlocks(a)).map((block) => block.id)).toEqual([null]);
    await editorLocator(a).locator(":scope > *").first().click();
    await expect.poll(() => caret(a)).toEqual({ block: 0, offset: 0 });
    const ime = await ctx.newCDPSession(a);
    for (const [steps, commit] of [
      [["ㅎ", "하", "한"], "한"],
      [["ㄱ", "그", "글"], "글"],
    ] as const) {
      for (const text of steps) {
        await ime.send("Input.imeSetComposition", { text, selectionStart: text.length, selectionEnd: text.length });
      }
      await ime.send("Input.insertText", { text: commit });
    }
    await expectBlocks(a, ["한글"]);
    await a.keyboard.press("Space");
    await expectBlocks(a, ["한글 "]);
    expect((await modelBlocks(a))[0]?.id).toEqual(expect.stringMatching(UUID_RE));
  } finally {
    await ctx.close();
  }
});

// --- The OS IME witness (opt-in) ---

const osIme = process.env.FVOCI_E2E_OS_IME === "1";
const env = { ...process.env } as Record<string, string>;
const xdotool = (...args: string[]) => execFileSync("xdotool", args, { env });
/** Real key presses; the IBus Hangul 2-set layout turns g k s r m f into ㅎ ㅏ ㄴ ㄱ ㅡ ㄹ. */
const keys = (...names: string[]) => xdotool("key", "--delay", "60", ...names);

/** A headed browser, whose X window receives the IME's key events. */
async function headedPage(baseUrl: string): Promise<{ browser: Browser; context: BrowserContext; page: Page }> {
  const browser = await chromium.launch({
    headless: false,
    env,
    args: ["--window-position=0,0", "--window-size=1200,900"],
  });
  const context = await browser.newContext({ baseURL: baseUrl, viewport: { width: 1180, height: 780 } });
  const page = await context.newPage();
  // The pointer stays over the window, so X keeps the keyboard focus there.
  xdotool("mousemove", "600", "450");
  return { browser, context, page };
}

/** A real X click on the right part of top-level block `index`, past its text
 * (the block gutter sits at its left edge). */
async function xClickBlock(page: Page, index: number): Promise<void> {
  const block = editorLocator(page).locator(":scope > *").nth(index);
  await block.evaluate((el) => el.scrollIntoView({ block: "center" }));
  const box = await block.boundingBox();
  if (!box) throw new Error(`block ${index} has no box`);
  const win = await page.evaluate(() => ({
    x: window.screenX + (window.outerWidth - window.innerWidth),
    y: window.screenY + (window.outerHeight - window.innerHeight),
  }));
  const x = win.x + box.x + box.width - 24;
  xdotool("mousemove", String(Math.round(x)), String(Math.round(win.y + box.y + box.height / 2)));
  xdotool("click", "1");
}

/** Puts the caret with real X input: a click, and Home/Right inside a word. */
async function xPlaceCaret(page: Page, where: Where): Promise<void> {
  const { block, offset } = CARET[where];
  await xClickBlock(page, block);
  if (where === "mid") {
    keys("Home", ...Array.from({ length: offset }, () => "Right"));
  }
  await expect.poll(() => caret(page)).toEqual({ block, offset });
}

test.describe("OS IME witness", () => {
  test.skip(!osIme, "opt-in: FVOCI_E2E_OS_IME=1 on an X display with IBus Hangul");

  test("control: the IME composes Korean in a plain contenteditable of the same browser", async ({ collabApp }) => {
    const a = await headedPage(collabApp.baseUrl);
    try {
      await a.page.setContent('<div contenteditable="true" style="min-height:80px"><p>첫 문단</p></div>');
      await a.page.locator("p").click();
      await a.page.keyboard.press("End");
      keys("g", "k", "s", "r", "m", "f", "space");
      // A contenteditable keeps a trailing space as U+00A0.
      await expect
        .poll(() => a.page.locator("p").evaluate((p) => (p.textContent ?? "").replace(/ /g, " ")))
        .toBe("첫 문단한글 ");
    } finally {
      await a.browser.close();
    }
  });

  for (const where of ["end", "empty", "mid"] as const) {
    for (const remote of [false, true]) {
      test(`real IBus Hangul keys (${where}${remote ? ", peer edit during preedit" : ""}) compose without a stray jamo`, async ({
        browser,
        collabApp,
      }) => {
        const a = await headedPage(collabApp.baseUrl);
        const ctxB = await newCollabContext(browser, collabApp.baseUrl);
        try {
          const b = await ctxB.newPage();
          await ensureCollabFixture(a.page);
          installCollabPeer(imePeer);
          await login(a.page, member.email, member.password);
          const doc = await seededDoc(a.page, `OS IME ${where}${remote ? " 원격" : ""}`);
          await a.page.goto(doc.url);
          await waitConnected(a.page);
          if (remote) {
            await login(b, imePeer.email, imePeer.password);
            await b.goto(doc.url);
            await waitConnected(b);
          }
          await expectBlocks(a.page, SEED);
          expect((await modelBlocks(a.page)).map((block) => block.id)).toEqual([null, null, null]);
          // The first composition right after placing the caret, with no pause.
          await xPlaceCaret(a.page, where);
          let texts = SEED;
          if (remote) {
            keys("g");
            await expect.poll(() => blockTexts(a.page)).toEqual(seedWith(where, "ㅎ"));
            texts = withRemote(SEED, peerBlock(where));
            await peerEdit(a.page, b, peerBlock(where), seedWith(where, "ㅎ", texts));
            keys("k", "s", "r", "m", "f", "space");
          } else {
            keys("g", "k", "s", "r", "m", "f", "space");
          }
          await expectBlocks(a.page, seedWith(where, "한글 ", texts));
          if (remote) await expectBlocks(b, seedWith(where, "한글 ", texts));
          const ids = (await modelBlocks(a.page)).map((block) => block.id);
          expect(ids[CARET[where].block]).toEqual(expect.stringMatching(UUID_RE));
        } finally {
          await ctxB.close();
          await a.browser.close();
        }
      });
    }
  }

  test("real IBus Hangul keys as the first input of a new document compose without a stray jamo", async ({
    collabApp,
  }) => {
    const a = await headedPage(collabApp.baseUrl);
    try {
      await ensureCollabFixture(a.page);
      await login(a.page, member.email, member.password);
      const doc = await createWikiDoc(a.page, "OS IME 새 문서");
      await a.page.goto(doc.url);
      await waitConnected(a.page);
      await expectBlocks(a.page, [""]);
      expect((await modelBlocks(a.page)).map((block) => block.id)).toEqual([null]);
      await xClickBlock(a.page, 0);
      await expect.poll(() => caret(a.page)).toEqual({ block: 0, offset: 0 });
      keys("g", "k", "s", "r", "m", "f", "space");
      await expectBlocks(a.page, ["한글 "]);
      expect((await modelBlocks(a.page))[0]?.id).toEqual(expect.stringMatching(UUID_RE));
    } finally {
      await a.browser.close();
    }
  });

  test("preedit Backspace, undo, redo, Save and a restart keep the composed text and the peer's", async ({
    browser,
    collabApp,
  }) => {
    test.setTimeout(180_000);
    const a = await headedPage(collabApp.baseUrl);
    const ctxB = await newCollabContext(browser, collabApp.baseUrl);
    try {
      const b = await ctxB.newPage();
      await ensureCollabFixture(a.page);
      installCollabPeer(imePeer);
      await login(a.page, member.email, member.password);
      await login(b, imePeer.email, imePeer.password);
      const doc = await seededDoc(a.page, "OS IME 흐름");
      for (const page of [a.page, b]) {
        await page.goto(doc.url);
        await waitConnected(page);
      }
      // The empty third paragraph has no id; B edits the second during A's preedit.
      await xPlaceCaret(a.page, "empty");
      keys("g");
      await expect.poll(() => blockTexts(a.page)).toEqual(["첫 문단", "둘째 문단", "ㅎ"]);
      await peerEdit(a.page, b, 1, ["첫 문단", "둘째 문단 원격", "ㅎ"]);
      keys("k", "s", "r", "m", "f", "space");
      await expectBlocks(a.page, ["첫 문단", "둘째 문단 원격", "한글 "]);
      await expectBlocks(b, ["첫 문단", "둘째 문단 원격", "한글 "]);

      // Backspace inside a preedit removes jamo only.
      keys("g", "k");
      await expect.poll(() => blockTexts(a.page)).toEqual(["첫 문단", "둘째 문단 원격", "한글 하"]);
      keys("BackSpace");
      await expect.poll(() => blockTexts(a.page)).toEqual(["첫 문단", "둘째 문단 원격", "한글 ㅎ"]);
      keys("BackSpace");
      await expect.poll(() => blockTexts(a.page)).toEqual(["첫 문단", "둘째 문단 원격", "한글 "]);
      keys("BackSpace");
      await expectBlocks(a.page, ["첫 문단", "둘째 문단 원격", "한글"]);
      await expectBlocks(b, ["첫 문단", "둘째 문단 원격", "한글"]);

      // Undo takes back only A's text (the Yjs undo manager groups by time), redo restores it.
      let undos = 0;
      while ((await blockTexts(a.page))[2] !== "") {
        expect(undos, "A's text is undone within six steps").toBeLessThan(6);
        const before = (await blockTexts(a.page))[2];
        keys("ctrl+z");
        undos += 1;
        await expect.poll(async () => (await blockTexts(a.page))[2]).not.toBe(before);
        expect((await blockTexts(a.page))[1]).toBe("둘째 문단 원격");
      }
      await expectBlocks(b, ["첫 문단", "둘째 문단 원격", ""]);
      for (let i = 0; i < undos; i += 1) keys("ctrl+shift+z");
      await expectBlocks(a.page, ["첫 문단", "둘째 문단 원격", "한글"]);
      await expectBlocks(b, ["첫 문단", "둘째 문단 원격", "한글"]);

      await a.page.getByRole("button", { name: "저장", exact: true }).click();
      await expect(a.page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });
      const body = await a.page.request.get(`/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`);
      expect(JSON.stringify((await body.json()).contentJson)).toContain("한글");

      await ctxB.close();
      await collabApp.recycle();
      const fresh = await newCollabContext(browser, collabApp.baseUrl);
      try {
        const c = await fresh.newPage();
        await login(c, member.email, member.password);
        await c.goto(doc.url);
        await waitConnected(c);
        await expectBlocks(c, ["첫 문단", "둘째 문단 원격", "한글"]);
      } finally {
        await fresh.close();
      }
    } finally {
      await ctxB.close().catch(() => undefined);
      await a.browser.close();
    }
  });
});
