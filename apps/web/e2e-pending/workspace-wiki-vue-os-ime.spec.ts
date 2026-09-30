import type { components } from "../src/generated/api";
/**
 * Opt-in OS IME witness for the Vue wiki page: real key events (XTEST, via
 * xdotool) through IBus Hangul (2-set) into headed Chromium, against the
 * owned server of the collaboration suite. Skipped unless FVOCI_E2E_OS_IME=1.
 *
 * Needs, on Linux: Xvfb, xdotool, dbus-run-session, ibus with the hangul
 * engine, and Playwright's Chromium. One way to run it (a private display
 * and D-Bus session; nothing is changed outside them):
 *
 *   export DISPLAY=:233; Xvfb :233 -screen 0 1280x1024x24 -nolisten tcp &
 *   HOME=$(mktemp -d) dbus-run-session -- bash -c '
 *     gsettings set org.freedesktop.ibus.engine.hangul initial-input-mode hangul
 *     ibus-daemon --daemonize --replace --xim --panel=disable; sleep 2
 *     ibus engine hangul
 *     export IBUS_ADDRESS=$(ibus address) GTK_IM_MODULE=ibus XMODIFIERS=@im=ibus
 *     export HOME=<your home> FVOCI_E2E_PENDING=1 FVOCI_E2E_OS_IME=1
 *     bash scripts/web-e2e-run-group.sh e2e-pending/workspace-wiki-vue-os-ime.spec.ts
 *     ibus exit'
 *
 * The CDP composition test in e2e/workspace-wiki-vue-flow.spec.ts is the
 * synthetic counterpart that runs in CI.
 */
import { execFileSync } from "node:child_process";
import { chromium, type Browser, type BrowserContext, type Page } from "@playwright/test";
import {
  createWikiDoc,
  ensureCollabFixture,
  expect,
  installCollabPeer,
  login,
  member,
  newCollabContext,
  peer,
  test,
  waitConnected,
  type WikiDoc,
} from "./collab-helpers";

test.skip(
  process.env.FVOCI_E2E_OS_IME !== "1",
  "opt-in: FVOCI_E2E_OS_IME=1 on an X display with IBus Hangul",
);

const env = { ...process.env } as Record<string, string>;
const xdotool = (...args: string[]) => execFileSync("xdotool", args, { env });
/** Real key presses; the IBus Hangul 2-set layout turns g k s r m f into ㅎ ㅏ ㄴ ㄱ ㅡ ㄹ. */
const keys = (...names: string[]) => xdotool("key", "--delay", "60", ...names);

const paragraph = (text?: string) => ({
  type: "paragraph",
  ...(text ? { content: [{ type: "text", text }] } : {}),
});

/** Top-level block texts without peer caret labels. */
async function blocks(page: Page): Promise<string[]> {
  return page.locator(".fvoci-editor .ProseMirror").evaluate((root) =>
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

/** A headed browser, whose X window receives the IME's key events. */
async function headedPage(
  baseUrl: string,
): Promise<{ browser: Browser; context: BrowserContext; page: Page }> {
  const browser = await chromium.launch({
    headless: false,
    env,
    args: ["--window-position=0,0", "--window-size=1200,900"],
  });
  const context = await browser.newContext({
    baseURL: baseUrl,
    viewport: { width: 1180, height: 780 },
  });
  const page = await context.newPage();
  // The pointer stays over the window, away from the text, so X keeps its focus.
  xdotool("mousemove", "600", "450");
  return { browser, context, page };
}

async function documentWith(
  page: Page,
  title: string,
  texts: (string | undefined)[],
): Promise<WikiDoc> {
  const doc = await createWikiDoc(page, title);
  const put = await page.request.put(
    `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`,
    {
      data: { contentJson: { type: "doc", content: texts.map(paragraph) } },
    },
  );
  expect(put.ok(), await put.text()).toBe(true);
  return doc;
}

test("control: the IME composes Korean in a plain contenteditable of the same browser", async ({
  collabApp,
}) => {
  const a = await headedPage(collabApp.baseUrl);
  try {
    await a.page.setContent(
      '<div contenteditable="true" style="min-height:80px"><p>첫 문단</p></div>',
    );
    await a.page.locator("p").click();
    await a.page.keyboard.press("End");
    keys("g", "k", "s", "r", "m", "f", "space");
    // A contenteditable keeps a trailing space as U+00A0.
    await expect
      .poll(() => a.page.locator("p").evaluate((p) => p.textContent.replace(/\u00a0/g, " ")))
      .toBe("첫 문단한글 ");
  } finally {
    await a.browser.close();
  }
});

// Known bug, in the React editor as well: the first composition after the
// caret is placed keeps its first jamo on its own ("ㅎ한글" for 한글), at the
// end of a text or in an empty paragraph, with or without a peer. The
// synthetic counterpart is the test.fail in e2e/workspace-wiki-vue-flow.spec.ts.
// Tracked in #258. Expected to fail until fixed; when it passes, drop test.fail.
test.fail(
  "the first composition after placing the caret leaves no stray jamo (known bug)",
  async ({ collabApp }) => {
    const a = await headedPage(collabApp.baseUrl);
    try {
      await ensureCollabFixture(a.page);
      await login(a.page, member.email, member.password);
      const doc = await documentWith(a.page, "OS IME 첫 조합", ["첫 문단"]);
      await a.page.goto(doc.url);
      await waitConnected(a.page);
      await a.page.locator(".fvoci-editor .ProseMirror > *").first().click();
      await a.page.keyboard.press("End");
      keys("g", "k", "s", "r", "m", "f", "space");
      await expect.poll(() => blocks(a.page), { timeout: 5_000 }).toEqual(["첫 문단한글 "]);
    } finally {
      await a.browser.close();
    }
  },
);

test("a composition survives a peer's edit; preedit Backspace, undo, redo, save and a restart keep it", async ({
  browser,
  collabApp,
}) => {
  test.setTimeout(180_000);
  const a = await headedPage(collabApp.baseUrl);
  const ctxB = await newCollabContext(browser, collabApp.baseUrl);
  const b = await ctxB.newPage();
  try {
    await ensureCollabFixture(a.page);
    installCollabPeer();
    await login(a.page, member.email, member.password);
    await login(b, peer.email, peer.password);
    const doc = await documentWith(a.page, "OS IME 증인", ["첫 문단", "둘째 문단", undefined]);
    for (const page of [a.page, b]) {
      await page.goto(doc.url);
      await waitConnected(page);
    }
    await expect(a.page.locator("#root[data-v-app]")).toHaveCount(1);

    // A typed space first: the next composition is not the first one after
    // placing the caret (the known bug above).
    await a.page.locator(".fvoci-editor .ProseMirror > *").nth(2).click();
    keys("space");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단", " "]);
    // A preedit in the third paragraph; B edits the second meanwhile.
    keys("g");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단", " ㅎ"]);
    await b.locator(".fvoci-editor .ProseMirror > *").nth(1).click();
    await b.keyboard.press("End");
    await b.keyboard.type(" 원격");
    await expect
      .poll(() => blocks(a.page), { timeout: 15_000 })
      .toEqual(["첫 문단", "둘째 문단 원격", " ㅎ"]);
    keys("k", "s", "r", "m", "f", "space");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단 원격", " 한글 "]);
    await expect
      .poll(() => blocks(b), { timeout: 15_000 })
      .toEqual(["첫 문단", "둘째 문단 원격", " 한글 "]);

    // Backspace inside a preedit removes jamo only.
    keys("g", "k");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단 원격", " 한글 하"]);
    keys("BackSpace");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단 원격", " 한글 ㅎ"]);
    keys("BackSpace");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단 원격", " 한글 "]);
    keys("BackSpace");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단 원격", " 한글"]);
    await expect
      .poll(() => blocks(b), { timeout: 15_000 })
      .toEqual(["첫 문단", "둘째 문단 원격", " 한글"]);

    // Undo takes back only A's text (the Yjs undo manager groups by time), redo restores it.
    let undos = 0;
    while ((await blocks(a.page))[2] !== "") {
      expect(undos, "A's text is undone within six steps").toBeLessThan(6);
      const before = (await blocks(a.page))[2];
      keys("ctrl+z");
      undos += 1;
      await expect.poll(async () => (await blocks(a.page))[2]).not.toBe(before);
      expect((await blocks(a.page))[1]).toBe("둘째 문단 원격");
    }
    await expect
      .poll(() => blocks(b), { timeout: 15_000 })
      .toEqual(["첫 문단", "둘째 문단 원격", ""]);
    for (let i = 0; i < undos; i += 1) keys("ctrl+shift+z");
    await expect.poll(() => blocks(a.page)).toEqual(["첫 문단", "둘째 문단 원격", " 한글"]);
    await expect
      .poll(() => blocks(b), { timeout: 15_000 })
      .toEqual(["첫 문단", "둘째 문단 원격", " 한글"]);

    await a.page.getByRole("button", { name: "저장", exact: true }).click();
    await expect(a.page.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15_000 });
    const body = await a.page.request.get(
      `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`,
    );
    expect(
      JSON.stringify(((await body.json()) as components["schemas"]["BodyResponse"]).contentJson),
    ).toContain(" 한글");

    await ctxB.close();
    await collabApp.recycle();
    const fresh = await newCollabContext(browser, collabApp.baseUrl);
    try {
      const c = await fresh.newPage();
      await login(c, member.email, member.password);
      await c.goto(doc.url);
      await waitConnected(c);
      await expect.poll(() => blocks(c)).toEqual(["첫 문단", "둘째 문단 원격", " 한글"]);
    } finally {
      await fresh.close();
    }
  } finally {
    await ctxB.close().catch(() => undefined);
    await a.browser.close();
  }
});
