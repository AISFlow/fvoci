import type { components } from "../src/generated/api";
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
import { writeFileSync } from "node:fs";
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
const SEED: [string, string, string] = ["첫 문단", "둘째 문단", ""];

type Where = "end" | "empty" | "mid";
/** Where each case composes: top-level block and character offset. */
const CARET: Record<Where, { block: number; offset: number }> = {
  end: { block: 0, offset: SEED[0].length },
  empty: { block: 2, offset: 0 },
  mid: { block: 1, offset: 1 },
};

/** The seed with `typed` inserted at the case's caret. */
function seedWith(where: Where, typed: string, texts: string[] = SEED): string[] {
  const { block, offset } = CARET[where];
  return texts.map((text, i) =>
    i === block ? text.slice(0, offset) + typed + text.slice(offset) : text,
  );
}

/** The block a peer edits meanwhile: never the one being composed in. */
const peerBlock = (where: Where) => (CARET[where].block === 1 ? 0 : 1);

async function seededDoc(page: Page, title: string): Promise<WikiDoc> {
  const doc = await createWikiDoc(page, title);
  const paragraph = (text: string) => ({
    type: "paragraph",
    ...(text ? { content: [{ type: "text", text }] } : {}),
  });
  const put = await page.request.put(
    `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`,
    {
      data: { contentJson: { type: "doc", content: SEED.map(paragraph) } },
    },
  );
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
      resolve(pos: number): { index(depth: number): number; parentOffset: number };
    };
    selection: { empty: boolean; $from: { index(depth: number): number; parentOffset: number } };
  };
  view: { posAtDOM(node: Node, offset: number): number; hasFocus(): boolean; focus(): void };
  on?(event: "transaction", handler: (event: ImeTransaction) => void): void;
  off?(event: "transaction", handler: (event: ImeTransaction) => void): void;
};

type ImeTransaction = {
  transaction: { selectionSet: boolean; docChanged: boolean; getMeta(key: string): unknown };
};
type ImeObservationHost = Window & {
  __fvociImeHomeObservation?: { finish(): unknown };
};

/** The editor model's top-level blocks: text and UniqueID. */
async function modelBlocks(
  page: Page,
  observeHome = false,
): Promise<Array<{ text: string; id: unknown }>> {
  return editorLocator(page).evaluate((root, observeHome) => {
    const editor = (root as HTMLElement & { editor?: LiveEditor }).editor;
    if (!editor) throw new Error("missing live editor");
    // Installed within this existing evaluation only for the mid+peer case:
    // no extra await before the original paragraph click and native Home.
    if (observeHome) {
      const host = root.ownerDocument.defaultView as ImeObservationHost;
      const doc = root.ownerDocument;
      const events: Array<Record<string, string | number | boolean | null>> = [];
      const started = performance.now();
      let dropped = 0;
      let setupError = false;
      let readError = false;
      let cleanupError = false;
      let transactionListening = false;
      let finished = false;
      const listeners: Array<[string, EventListener, boolean]> = [];
      const record = (event: string, input?: Event, tx?: ImeTransaction) => {
        if (finished) return;
        try {
          const selection = editor.state.selection;
          const native = doc.getSelection();
          let browserBlock = null;
          let browserOffset = null;
          if (native?.isCollapsed && native.anchorNode && root.contains(native.anchorNode)) {
            const pos = editor.state.doc.resolve(
              editor.view.posAtDOM(native.anchorNode, native.anchorOffset),
            );
            browserBlock = pos.index(0);
            browserOffset = pos.parentOffset;
          }
          const key = input instanceof KeyboardEvent ? input.key : "none";
          const row = {
            event,
            key: ["Home", "End", "ArrowRight", "none"].includes(key) ? key : "other",
            elapsedMs: Math.min(60_000, Math.max(0, Math.round(performance.now() - started))),
            phase: input?.eventPhase ?? 0,
            trusted: input?.isTrusted ?? null,
            prevented: input?.defaultPrevented ?? null,
            composing: input instanceof KeyboardEvent ? input.isComposing : null,
            focused: editor.view.hasFocus(),
            activeInside: !!doc.activeElement && root.contains(doc.activeElement),
            rootConnected: root.isConnected,
            sameEditor: (root as HTMLElement & { editor?: LiveEditor }).editor === editor,
            modelBlock: selection.empty ? selection.$from.index(0) : null,
            modelOffset: selection.empty ? selection.$from.parentOffset : null,
            browserBlock,
            browserOffset,
            selectionSet: tx?.transaction.selectionSet ?? null,
            docChanged: tx?.transaction.docChanged ?? null,
            compositionMeta: tx ? tx.transaction.getMeta("composition") !== undefined : null,
          };
          if (events.length === 64) {
            events.shift();
            dropped = Math.min(1_000_000, dropped + 1);
          }
          events.push(row);
        } catch {
          readError = true;
        }
      };
      const transaction = (event: ImeTransaction) => {
        record("transaction", undefined, event);
      };
      const finish = () => {
        if (!finished) {
          record("finish");
          finished = true;
          for (const [name, listener, capture] of listeners) {
            try {
              doc.removeEventListener(name, listener, capture);
            } catch {
              cleanupError = true;
            }
          }
          if (transactionListening) {
            try {
              const off = editor.off?.bind(editor);
              if (off) off("transaction", transaction);
              else cleanupError = true;
            } catch {
              cleanupError = true;
            }
          }
        }
        return {
          schema: 1,
          setupError,
          readError,
          cleanupError,
          transactionAvailable: !!editor.on && !!editor.off,
          dropped,
          events,
        };
      };
      try {
        host.__fvociImeHomeObservation = { finish };
        for (const name of [
          "pointerdown",
          "pointerup",
          "focus",
          "blur",
          "keydown",
          "keyup",
          "selectionchange",
        ]) {
          for (const capture of [true, false]) {
            const listener: EventListener = (event) => {
              if (
                name !== "selectionchange" &&
                !(event.target instanceof Node && root.contains(event.target))
              )
                return;
              record(name, event);
            };
            // Track before registration so partial setup still cleans up.
            listeners.push([name, listener, capture]);
            doc.addEventListener(name, listener, { capture, passive: true });
          }
        }
        if (editor.on && editor.off) {
          transactionListening = true;
          editor.on("transaction", transaction);
        }
        record("installed");
      } catch {
        setupError = true;
        finish();
      }
    }
    const out: Array<{ text: string; id: unknown }> = [];
    editor.state.doc.forEach((node) => out.push({ text: node.textContent, id: node.attrs.id }));
    return out;
  }, observeHome);
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

/** Navigation is complete only when PM and the focused browser caret agree.
 * Home/End move the native Selection before selectionchange reaches PM. A
 * pending focus repair can restore PM's old paragraph end in that gap, so the
 * next ArrowRight enters the following empty paragraph instead of the word. */
async function expectCaret(page: Page, block: number, offset: number): Promise<void> {
  await expect
    .poll(() =>
      editorLocator(page).evaluate((root) => {
        const editor = (root as HTMLElement & { editor?: LiveEditor }).editor;
        if (!editor) throw new Error("missing live editor");
        const selection = editor.state.selection;
        const native = root.ownerDocument.getSelection();
        let browser = null;
        if (native?.isCollapsed && native.anchorNode && root.contains(native.anchorNode)) {
          const pos = editor.state.doc.resolve(
            editor.view.posAtDOM(native.anchorNode, native.anchorOffset),
          );
          browser = { block: pos.index(0), offset: pos.parentOffset };
        }
        return {
          focused: editor.view.hasFocus(),
          model: selection.empty
            ? { block: selection.$from.index(0), offset: selection.$from.parentOffset }
            : null,
          browser,
        };
      }),
    )
    .toEqual({ focused: true, model: { block, offset }, browser: { block, offset } });
}

async function navigateCaret(
  page: Page,
  key: string,
  block: number,
  offset: number,
): Promise<void> {
  await page.keyboard.press(key);
  await expectCaret(page, block, offset);
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
    await navigateCaret(page, "Home", block, 0);
    for (let i = 0; i < offset; i += 1) await navigateCaret(page, "ArrowRight", block, i + 1);
  } else {
    await navigateCaret(page, "End", block, offset);
  }
  await expect.poll(() => caret(page)).toEqual({ block, offset });
}

test("native Home survives the pending editor focus repair before ArrowRight", async ({
  browser,
  collabApp,
}) => {
  const ctx = await newCollabContext(browser, collabApp.baseUrl);
  try {
    const page = await ctx.newPage();
    await ensureCollabFixture(page);
    await login(page, member.email, member.password);
    const doc = await seededDoc(page, "IME native caret focus repair");
    await page.goto(doc.url);
    await waitConnected(page);
    await expectBlocks(page, SEED);

    // Control scheduling of the installed PM focus-repair callback, as in
    // workspace-wiki-vue-controls. Use real click/keyboard events and no sleeps,
    // model selection writes, or selectionchange observer replacement.
    await editorLocator(page).evaluate((element) => {
      const root = element as HTMLElement & { editor: LiveEditor };
      const snapshot = () => {
        const native = document.getSelection();
        if (!native?.anchorNode) throw new Error("native caret fixture has no anchor node");
        const model = root.editor.state.selection.$from;
        const browser = root.editor.state.doc.resolve(
          root.editor.view.posAtDOM(native.anchorNode, native.anchorOffset),
        );
        return {
          model: { block: model.index(0), offset: model.parentOffset },
          browser: { block: browser.index(0), offset: browser.parentOffset },
        };
      };
      const gate = {
        captured: false,
        delivered: false,
        before: null as ReturnType<typeof snapshot> | null,
        after: null as ReturnType<typeof snapshot> | null,
      };
      Object.assign(window, { __imeFocusRepair: gate });
      const nativeTimeout = window.setTimeout.bind(window);
      let focusing = false;
      let repair: (() => void) | undefined;
      let timer: number | undefined;
      root.addEventListener(
        "focus",
        () => {
          focusing = true;
        },
        { capture: true, once: true },
      );
      root.addEventListener(
        "focusin",
        () => {
          focusing = false;
        },
        { once: true },
      );
      window.setTimeout = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
        // Pinned prosemirror-view schedules this during the root focus event.
        if (focusing && delay === 20 && typeof callback === "function") {
          if (repair) throw new Error("multiple editor focus-repair tasks");
          gate.captured = true;
          repair = () => {
            Reflect.apply(callback, undefined, args);
          };
          timer = nativeTimeout(() => {}, delay);
          return timer;
        }
        return nativeTimeout(callback, delay, ...args);
      }) as typeof window.setTimeout;
      const deliver = (event: KeyboardEvent) => {
        if (event.key !== "Home" || event.shiftKey) return;
        window.setTimeout = nativeTimeout;
        document.removeEventListener("keyup", deliver);
        window.clearTimeout(timer);
        if (!repair) throw new Error("missing pending editor focus repair");
        gate.before = snapshot();
        repair();
        gate.delivered = true;
        gate.after = snapshot();
      };
      // After the product keyup handler, before async selectionchange.
      document.addEventListener("keyup", deliver);
    });
    await editorLocator(page).locator(":scope > *").nth(1).click();
    await expectCaret(page, 1, SEED[1].length);
    await page.keyboard.press("Home");
    const gate = await page.evaluate(
      () => (window as unknown as { __imeFocusRepair: unknown }).__imeFocusRepair,
    );
    await test.info().attach("native-home-focus-repair", {
      body: JSON.stringify(gate),
      contentType: "application/json",
    });
    const expected = { model: { block: 1, offset: 0 }, browser: { block: 1, offset: 0 } };
    expect(gate).toEqual({ captured: true, delivered: true, before: expected, after: expected });
    // Deliberately no fixture navigation barrier between native keys.
    await page.keyboard.press("ArrowRight");
    await expectCaret(page, 1, 1);
    await expectBlocks(page, SEED);
    expect((await modelBlocks(page)).map((node) => node.id)).toEqual([null, null, null]);
  } finally {
    await ctx.close();
  }
});

/** A peer appends " 원격" to a block and A receives it. */
async function peerEdit(a: Page, b: Page, block: number, expected: string[]): Promise<void> {
  await editorLocator(b).locator(":scope > *").nth(block).click();
  await b.keyboard.press("End");
  await b.keyboard.type(" 원격");
  await expect.poll(() => blockTexts(a), { timeout: 15_000 }).toEqual(expected);
}

const withRemote = (texts: string[], block: number) =>
  texts.map((text, i) => (i === block ? `${text} 원격` : text));

/** Only scalar allowlisted observations leave the page; diagnostic failure
 * never masks the original oracle. The original context-close await owns this
 * finalization after the test body, including its failure path. */
function closeObservedImeContext(ctx: BrowserContext, enabled: boolean): Promise<void> {
  if (!enabled) return ctx.close();
  return Promise.resolve()
    .then(() =>
      ctx.pages()[0]?.evaluate(() => {
        const host = window as ImeObservationHost;
        const result = host.__fvociImeHomeObservation?.finish();
        delete host.__fvociImeHomeObservation;
        return result;
      }),
    )
    .then((raw) => {
      const data = raw as Record<string, unknown> | undefined;
      const number = (value: unknown, max: number) =>
        typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= max
          ? value
          : null;
      const boolean = (value: unknown) => (typeof value === "boolean" ? value : null);
      const rows = Array.isArray(data?.events) ? data.events.slice(-64) : [];
      const events = rows.map((rawRow: unknown) => {
        const row = (rawRow ?? {}) as Record<string, unknown>;
        const out: Record<string, string | number | boolean | null> = {
          event: [
            "installed",
            "finish",
            "pointerdown",
            "pointerup",
            "focus",
            "blur",
            "keydown",
            "keyup",
            "selectionchange",
            "transaction",
          ].includes(row.event as string)
            ? (row.event as string)
            : "invalid",
          key: ["Home", "End", "ArrowRight", "none", "other"].includes(row.key as string)
            ? (row.key as string)
            : "invalid",
          elapsedMs: number(row.elapsedMs, 60_000),
          phase: number(row.phase, 3),
        };
        for (const key of [
          "trusted",
          "prevented",
          "composing",
          "focused",
          "activeInside",
          "rootConnected",
          "sameEditor",
          "selectionSet",
          "docChanged",
          "compositionMeta",
        ])
          out[key] = boolean(row[key]);
        for (const key of ["modelBlock", "modelOffset", "browserBlock", "browserOffset"])
          out[key] = number(row[key], 1_000_000);
        return out;
      });
      const output = {
        schema: 1,
        available: data?.schema === 1,
        setupError: boolean(data?.setupError),
        readError: boolean(data?.readError),
        cleanupError: boolean(data?.cleanupError),
        transactionAvailable: boolean(data?.transactionAvailable),
        dropped: number(data?.dropped, 1_000_000),
        truncated:
          (number(data?.dropped, 1_000_000) ?? 0) > 0 ||
          rows.length !== (data?.events as unknown[] | undefined)?.length,
        events,
      };
      const path = test.info().outputPath("ime-home-observations.json");
      const body = JSON.stringify(output);
      if (Buffer.byteLength(body, "utf8") > 32_768) return;
      writeFileSync(path, body, { flag: "wx", mode: 0o600 });
      return test.info().attach("ime-home-observations", { path, contentType: "application/json" });
    })
    .catch(() => undefined)
    .finally(() => ctx.close());
}

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
        expect((await modelBlocks(a, where === "mid" && remote)).map((block) => block.id)).toEqual([
          null,
          null,
          null,
        ]);
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
        let texts: string[] = SEED;
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
        await closeObservedImeContext(ctxA, where === "mid" && remote);
        await ctxB.close();
      }
    });
  }
}

test("CDP composition as the first input of a new document leaves no stray jamo", async ({
  browser,
  collabApp,
}) => {
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
        await ime.send("Input.imeSetComposition", {
          text,
          selectionStart: text.length,
          selectionEnd: text.length,
        });
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
  // The pointer stays over the window, so X keeps the keyboard focus there.
  xdotool("mousemove", "600", "450");
  return { browser, context, page };
}

/** A real X click on the right part of top-level block `index`, past its text
 * (the block gutter sits at its left edge). */
async function xClickBlock(page: Page, index: number): Promise<void> {
  const block = editorLocator(page).locator(":scope > *").nth(index);
  await block.evaluate((el) => {
    el.scrollIntoView({ block: "center" });
  });
  const box = await block.boundingBox();
  if (!box) throw new Error(`block ${String(index)} has no box`);
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
          let texts: string[] = SEED;
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
      await expect(a.page.locator('[data-collab-persisted="true"]')).toBeVisible({
        timeout: 15_000,
      });
      const body = await a.page.request.get(
        `/api/v1/workspaces/${doc.workspaceId}/documents/${doc.id}/body`,
      );
      expect(
        JSON.stringify(((await body.json()) as components["schemas"]["BodyResponse"]).contentJson),
      ).toContain("한글");

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
