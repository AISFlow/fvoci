// The Vue wiki page's editing controls (#261): the block gutter and its
// block menu, the table handles, the code-block chrome, the selection
// bubble's formatting controls and the mobile toolbar, with the keyboard
// model of the React editor's menus (react/menu-keyboard.ts). Runs against
// the production build the Rust server serves, with the real PostgreSQL,
// Meilisearch and collab engine of the e2e group.
import { expect, type Page, test } from "@playwright/test";
import { login, watchCspViolations } from "./helpers";
import {
  admin,
  blockAt,
  blockTexts,
  caretAtEndOf,
  createDoc,
  editorOf,
  expectBlocks,
  focused,
  newSignedInPage,
  openDoc,
  save,
  savedBody,
  setupInstance,
  watchIconRequests,
  workspaceId,
} from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });

test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
});

/** Hovers top-level block `index` so the drag handle moves beside it. */
async function hoverBlock(page: Page, index: number): Promise<void> {
  const block = blockAt(page, index);
  await block.hover({ position: { x: 8, y: 4 } });
  const box = await block.boundingBox();
  const handle = page.locator(".fvoci-gutter");
  // The handle sits beside the hovered block (the plugin positions it on a frame).
  await expect
    .poll(async () => {
      const at = await handle.boundingBox();
      return Boolean(box && at && Math.abs(at.y - box.y) < box.height);
    })
    .toBe(true);
}

async function openBlockMenu(page: Page, index: number): Promise<void> {
  await hoverBlock(page, index);
  await page.locator('[data-gutter="drag"]').click();
  await expect(page.getByRole("menu", { name: "블록" })).toBeVisible();
}

/** The caret at the end of block `index`, with the pointer off the editor
 * and the drag handle (which Tab would reach first) hidden: the handle
 * follows the pointer on the next frame, and a key press hides it. */
async function keyboardCaretAtEndOf(page: Page, index: number): Promise<void> {
  await blockAt(page, index).click();
  await page.mouse.move(1, 1);
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
  );
  await page.keyboard.press("End");
  await expect(page.locator(".fvoci-gutter")).toBeHidden();
}

function blockMenu(page: Page) {
  return page.getByRole("menu", { name: "블록" });
}

test("the block gutter adds, converts, duplicates, moves, colours and deletes blocks", async ({
  page,
}) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "거터", { markdown: "첫째\n\n둘째\n\n셋째\n" });
  await openDoc(page, doc.path);
  await expectBlocks(page, ["첫째", "둘째", "셋째"]);

  // Hovering a block shows "+" and the drag handle beside it.
  await hoverBlock(page, 1);
  await expect(page.locator('[data-gutter="plus"]')).toBeVisible();
  await expect(page.locator('[data-gutter="drag"]')).toBeVisible();
  await expect(page.locator('[data-gutter="plus"]')).toHaveAccessibleName("블록 추가");
  await expect(page.locator('[data-gutter="drag"]')).toHaveAccessibleName("블록 이동");

  // "+" on a filled block adds an empty block below it with the slash menu open.
  await page.locator('[data-gutter="plus"]').click();
  await expect(page.locator(".fvoci-suggestion")).toBeVisible();
  await expectBlocks(page, ["첫째", "둘째", "/", "셋째"]);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Backspace");
  await expectBlocks(page, ["첫째", "둘째", "셋째"]);

  // The drag handle opens the block menu; it enters at its first item and
  // ↑/↓/Home/End move through the 17 items.
  await openBlockMenu(page, 0);
  const menu = blockMenu(page);
  const items = menu.getByRole("menuitem");
  await expect(items).toHaveCount(17);
  await expect(page.locator('[data-gutter="drag"]')).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator('[data-gutter="drag"]')).toHaveAttribute(
    "aria-controls",
    (await menu.getAttribute("id"))!,
  );
  await expect(items.first()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(items.nth(1)).toBeFocused();
  await page.keyboard.press("End");
  await expect(items.last()).toBeFocused();
  await page.keyboard.press("Home");
  await expect(items.first()).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(items.last()).toBeFocused();

  // Escape closes it and gives focus back to the handle.
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  expect(await focused(page)).toBe("button:블록 이동");

  // Convert: the first block becomes a heading; the editor keeps the focus.
  await openBlockMenu(page, 0);
  await blockMenu(page).getByRole("menuitem", { name: "제목 1" }).click();
  await expect(blockMenu(page)).toHaveCount(0);
  await expect(editorOf(page).locator(":scope > h1")).toHaveText("첫째");
  await expect.poll(() => focused(page)).toBe("editor");

  // Move down, then up again.
  await openBlockMenu(page, 1);
  await blockMenu(page).getByRole("menuitem", { name: "아래" }).click();
  await expectBlocks(page, ["첫째", "셋째", "둘째"]);
  await openBlockMenu(page, 2);
  await blockMenu(page).getByRole("menuitem", { name: "위" }).click();
  await expectBlocks(page, ["첫째", "둘째", "셋째"]);

  // Duplicate, then delete the copy.
  await openBlockMenu(page, 2);
  await blockMenu(page).getByRole("menuitem", { name: "복제" }).click();
  await expectBlocks(page, ["첫째", "둘째", "셋째", "셋째"]);
  await openBlockMenu(page, 3);
  await blockMenu(page).getByRole("menuitem", { name: "삭제" }).click();
  await expectBlocks(page, ["첫째", "둘째", "셋째"]);

  // Colour: the block's text takes the danger colour.
  await openBlockMenu(page, 1);
  await blockMenu(page).getByRole("menuitem", { name: "색", exact: true }).click();
  await expect(blockAt(page, 1).locator("span")).toHaveCSS("color", /rgb/);
  expect(
    await blockAt(page, 1)
      .locator("span")
      .evaluate((span) => (span as HTMLElement).style.color),
  ).toBe("var(--destructive)");

  // Copy link: "#<block id>" on the clipboard, and the menu closes. (Seeded
  // blocks get an id once edited; the converted heading has one.)
  await expect(blockAt(page, 0)).toHaveAttribute("data-id", /\S+/);
  const blockId = await blockAt(page, 0).getAttribute("data-id");
  await openBlockMenu(page, 0);
  await blockMenu(page).getByRole("menuitem", { name: "블록 링크 복사" }).click();
  await expect(blockMenu(page)).toHaveCount(0);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(`#${blockId}`);

  // A context menu on a block (nothing selected) opens the same menu there.
  await caretAtEndOf(page, 2);
  await blockAt(page, 2).click({ button: "right" });
  await expect(blockMenu(page)).toBeVisible();
  await expect(blockMenu(page).getByRole("menuitem").first()).toBeFocused();
  await blockMenu(page).getByRole("menuitem", { name: "인용" }).click();
  await expect(editorOf(page).locator(":scope > blockquote")).toHaveText("셋째");

  // Keyboard: Tab from the editor reaches the gutter's keyboard button,
  // which opens the menu for the caret's block.
  await keyboardCaretAtEndOf(page, 1);
  await page.keyboard.press("Tab");
  expect(await focused(page)).toBe("button:블록 이동");
  await page.keyboard.press("Enter");
  await expect(blockMenu(page)).toBeVisible();
  await expect(blockMenu(page).getByRole("menuitem").first()).toBeFocused();
  // Tab leaves the menu: it closes and focus moves on from the button.
  await page.keyboard.press("Tab");
  await expect(blockMenu(page)).toHaveCount(0);
  expect(await focused(page)).not.toMatch(/^menuitem/);
  expect(await focused(page)).not.toBe("body");
  // Enter on the keyboard button again, then a heading by keyboard.
  await keyboardCaretAtEndOf(page, 1);
  await page.keyboard.press("Tab");
  expect(await focused(page)).toBe("button:블록 이동");
  await page.keyboard.press("Enter");
  await expect(blockMenu(page).getByRole("menuitem").first()).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(blockMenu(page).getByRole("menuitem", { name: "제목 2" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(editorOf(page).locator(":scope > h2")).toHaveText("둘째");

  // The quote at the end got the editor's trailing paragraph after it.
  await expectBlocks(page, ["첫째", "둘째", "셋째", ""]);
  await save(page);
  const body = await savedBody(page.request, wsId, doc.id);
  expect(body.content?.map((node) => node.type)).toEqual([
    "heading",
    "heading",
    "blockquote",
    "paragraph",
  ]);
  expect(JSON.stringify(body)).toContain('"color":"var(--destructive)"');
  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

const paragraph = (text: string) =>
  text.length > 0
    ? { type: "paragraph", content: [{ type: "text", text }] }
    : { type: "paragraph" };
const tableCell = (type: "tableCell" | "tableHeader", text: string, colwidth?: number[]) => ({
  type,
  attrs: colwidth ? { colwidth } : {},
  content: [paragraph(text)],
});

/** Row count and each row's cell tags (th/td) of the editor's table. */
async function tableShape(page: Page): Promise<string[][]> {
  return editorOf(page)
    .locator("table")
    .evaluate((table) =>
      [...table.querySelectorAll("tr")].map((row) =>
        [...row.children].map((cell) => `${cell.tagName.toLowerCase()}:${cell.textContent ?? ""}`),
      ),
    );
}

function tableMenu(page: Page) {
  return page.getByRole("menu", { name: "표" });
}

async function openTableMenu(page: Page, handle: "table" | "col" | "row"): Promise<void> {
  await page.locator(`[data-table-handle="${handle}"]`).click();
  await expect(tableMenu(page)).toBeVisible();
}

test("the table handles insert, delete, move, format, merge and delete through the table menu", async ({
  page,
}) => {
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "표 손잡이", {
    json: {
      type: "doc",
      content: [
        paragraph("앞"),
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [tableCell("tableHeader", "A", [120]), tableCell("tableHeader", "B", [80])],
            },
            {
              type: "tableRow",
              content: [tableCell("tableCell", "1", [120]), tableCell("tableCell", "2", [80])],
            },
          ],
        },
        paragraph("뒤"),
      ],
    },
  });
  const seeded = JSON.stringify(await savedBody(page.request, wsId, doc.id));
  expect(seeded).toContain('"colwidth":[120]');
  await openDoc(page, doc.path);
  const cellText = (text: string) =>
    editorOf(page)
      .locator("td, th")
      .filter({ hasText: new RegExp(`^${text}$`) });

  // No handles while the caret is outside a table.
  await caretAtEndOf(page, 0);
  await expect(page.locator("[data-table-handles]")).toHaveCount(0);
  await cellText("1").click();
  const handles = page.locator("[data-table-handles]");
  await expect(handles).toBeVisible();
  for (const name of ["표 손잡이", "열 손잡이", "행 손잡이", "열 추가", "행 추가"]) {
    await expect(handles.getByRole("button", { name })).toBeVisible();
  }
  // The handles sit on the table's top edge.
  const tableBox = await editorOf(page).locator("table").boundingBox();
  const handlesBox = await handles.boundingBox();
  expect(Math.abs((handlesBox?.y ?? 0) - (tableBox?.y ?? 1000))).toBeLessThan(2);

  // "+" adds a column after the caret's and a row after the caret's.
  await handles.getByRole("button", { name: "열 추가" }).click();
  await expect
    .poll(() => tableShape(page))
    .toEqual([
      ["th:A", "th:", "th:B"],
      ["td:1", "td:", "td:2"],
    ]);
  await cellText("1").click();
  await handles.getByRole("button", { name: "행 추가" }).click();
  await expect.poll(async () => (await tableShape(page)).length).toBe(3);

  // The column handle's menu: 19 items, entered at the first; delete the
  // column and the row the caret is in (the added ones).
  await editorOf(page).locator("td").nth(4).click();
  await openTableMenu(page, "col");
  await expect(tableMenu(page).getByRole("menuitem")).toHaveCount(19);
  await expect(tableMenu(page).getByRole("menuitem").first()).toBeFocused();
  await expect(page.locator('[data-table-handle="col"]')).toHaveAttribute(
    "aria-controls",
    (await tableMenu(page).getAttribute("id"))!,
  );
  await tableMenu(page).getByRole("menuitem", { name: "열 삭제" }).click();
  await expect(tableMenu(page)).toHaveCount(0);
  await expect.poll(async () => (await tableShape(page))[0]).toEqual(["th:A", "th:B"]);
  await editorOf(page).locator("tr").nth(2).locator("td").first().click();
  await openTableMenu(page, "row");
  await tableMenu(page).getByRole("menuitem", { name: "행 삭제" }).click();
  await expect
    .poll(() => tableShape(page))
    .toEqual([
      ["th:A", "th:B"],
      ["td:1", "td:2"],
    ]);

  // Header row off and on; header column on.
  await cellText("1").click();
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "헤더 행" }).click();
  await expect
    .poll(() => tableShape(page))
    .toEqual([
      ["td:A", "td:B"],
      ["td:1", "td:2"],
    ]);
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "헤더 행" }).click();
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "헤더 열" }).click();
  await expect
    .poll(() => tableShape(page))
    .toEqual([
      ["th:A", "th:B"],
      ["th:1", "td:2"],
    ]);

  // Cell alignment and background.
  await cellText("2").click();
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "가운데" }).click();
  await expect(cellText("2").locator("p")).toHaveCSS("text-align", "center");
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "배경 액센트" }).click();
  await expect(cellText("2")).toHaveAttribute("data-background", "var(--accent)");

  // Equal column widths drop the stored widths.
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "열 너비 균등" }).click();

  // Merge two selected cells, then split them again.
  await cellText("A").click();
  await cellText("B").click({ modifiers: ["Shift"] });
  await expect(editorOf(page).locator(".selectedCell")).toHaveCount(2);
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "병합" }).click();
  await expect(editorOf(page).locator('th[colspan="2"]')).toHaveCount(1);
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "분할" }).click();
  await expect(editorOf(page).locator('th[colspan="2"]')).toHaveCount(0);
  await expect.poll(async () => (await tableShape(page))[0]).toEqual(["th:AB", "th:"]);

  // Move the table down past "뒤", then back up. (The split left a cell
  // selection, whose bubble covers the table: collapse it first.)
  await caretAtEndOf(page, 0);
  await cellText("1").click();
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "표 아래로" }).click();
  await expect(tableMenu(page)).toHaveCount(0);
  // A table at the end gets the editor's trailing paragraph after it.
  await expectBlocks(page, ["앞", "뒤", "AB12", ""]);
  await cellText("1").click();
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "표 위로" }).click();
  await expectBlocks(page, ["앞", "AB12", "뒤", ""]);

  await save(page);
  const body = await savedBody(page.request, wsId, doc.id);
  const table = body.content?.find((node) => node.type === "table");
  const cells = table?.content?.flatMap((row) => row.content ?? []) ?? [];
  expect(cells.map((cell) => cell.type)).toEqual([
    "tableHeader",
    "tableHeader",
    "tableHeader",
    "tableCell",
  ]);
  expect(cells[3]?.attrs).toMatchObject({ background: "var(--accent)" });
  expect(cells[3]?.content?.[0]?.attrs).toMatchObject({ textAlign: "center" });
  // Equal widths: no cell keeps the stored widths (a null width is left out).
  expect(cells.map((cell) => cell.attrs?.colwidth ?? null)).toEqual([null, null, null, null]);

  // Delete the table.
  await cellText("1").click();
  await openTableMenu(page, "table");
  await tableMenu(page).getByRole("menuitem", { name: "표 삭제" }).click();
  await expect(editorOf(page).locator("table")).toHaveCount(0);
  await expect(page.locator("[data-table-handles]")).toHaveCount(0);
  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

test("the code-block chrome sets the language, copies, and shows line numbers, wrap, fold and highlighted lines", async ({
  page,
}) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  const csp = watchCspViolations(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "코드", {
    json: {
      type: "doc",
      content: [
        paragraph("앞"),
        {
          type: "codeBlock",
          attrs: { language: "typescript", highlightLines: [1] },
          content: [{ type: "text", text: "one\ntwo\nthree" }],
        },
        {
          type: "codeBlock",
          attrs: { language: "diff" },
          content: [{ type: "text", text: "-old\n+new" }],
        },
        paragraph(""),
      ],
    },
  });
  await openDoc(page, doc.path);
  const chrome = page.locator(".fvoci-code-chrome");
  const host = page.locator(".fvoci-editor");

  // No chrome outside a code block.
  await caretAtEndOf(page, 0);
  await expect(chrome).toHaveCount(0);

  // A ```ts fence typed into an empty paragraph makes a TypeScript block;
  // its grammar loads lazily and paints tokens.
  await blockAt(page, 3).click();
  await page.keyboard.type("```ts ");
  const code = editorOf(page)
    .locator("pre")
    .filter({ has: page.locator("code.language-typescript") })
    .last();
  await expect(code).toBeVisible();
  await code.click();
  await page.keyboard.type("const a = 1;");
  await expect.poll(() => code.locator("code span").count()).toBeGreaterThanOrEqual(2);
  const language = chrome.getByLabel("언어");
  await expect(language).toHaveValue("typescript");

  // The language is an editor command (it reaches the saved body).
  await language.selectOption("python");
  await expect(editorOf(page).locator("pre").last().locator("code")).toHaveClass(/language-python/);

  // Copy puts the block's text on the clipboard; a failed copy says so.
  await chrome.getByRole("button", { name: "복사" }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("const a = 1;");
  await page.evaluate(() => {
    navigator.clipboard.writeText = () =>
      Promise.reject(new DOMException("denied", "NotAllowedError"));
  });
  await chrome.getByRole("button", { name: "복사" }).click();
  await expect(chrome.getByRole("alert")).toHaveText("복사하지 못했습니다. 다시 시도해 주세요.");

  // Line numbers and wrap are per block view options on the editor host.
  const linenos = chrome.getByRole("button", { name: "줄번호" });
  await expect(linenos).toHaveAttribute("aria-pressed", "false");
  await linenos.click();
  await expect(linenos).toHaveAttribute("aria-pressed", "true");
  await expect(chrome.locator(".fvoci-code-linenos")).toHaveText("1\n");
  const wrap = chrome.getByRole("button", { name: "줄바꿈" });
  await wrap.click();
  await expect(host).toHaveAttribute("data-code-wrap", "true");
  await expect(editorOf(page).locator("pre").last()).toHaveCSS("white-space", "pre-wrap");

  // Fold is offered from nine lines on.
  await expect(chrome.getByRole("button", { name: "접기" })).toHaveCount(0);
  for (let i = 2; i <= 9; i += 1) {
    await page.keyboard.press("Enter");
    await page.keyboard.type(`line ${i}`);
  }
  const fold = chrome.getByRole("button", { name: "접기" });
  await fold.click();
  await expect(host).toHaveAttribute("data-code-folded", "true");
  await expect(editorOf(page).locator("pre").last()).toHaveCSS("max-height", "128px");

  // Leaving the block removes the chrome and the host's view options.
  await caretAtEndOf(page, 0);
  await expect(chrome).toHaveCount(0);
  await expect(host).not.toHaveAttribute("data-code-wrap", /.*/);
  await expect(host).not.toHaveAttribute("data-code-folded", /.*/);

  // Highlighted lines ({1}) and diff lines get line backgrounds.
  await editorOf(page).locator("pre").nth(0).click();
  await expect(chrome.locator('[data-hl="meta"]')).toHaveCount(1);
  await expect(chrome.getByRole("button", { name: "줄번호" })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  await editorOf(page).locator("pre").nth(1).click();
  await expect(chrome.locator('[data-hl="del"]')).toHaveCount(1);
  await expect(chrome.locator('[data-hl="add"]')).toHaveCount(1);
  // The typed block kept its own options.
  await editorOf(page).locator("pre").nth(2).click();
  await expect(chrome.getByRole("button", { name: "줄번호" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await save(page);
  const body = await savedBody(page.request, wsId, doc.id);
  const blocks = body.content?.filter((node) => node.type === "codeBlock") ?? [];
  expect(blocks.map((block) => block.attrs?.language)).toEqual(["typescript", "diff", "python"]);
  expect(blocks[2]?.content?.[0]?.text).toContain("const a = 1;\nline 2");
  expect(csp).toEqual([]);
});

/** Selects the text of top-level block `index` with the keyboard. */
async function selectBlockText(page: Page, index: number): Promise<void> {
  await caretAtEndOf(page, index);
  await page.keyboard.press("Shift+Home");
}

test("the selection bubble formats text and its menus and popovers follow the menu keyboard model", async ({
  page,
}) => {
  const csp = watchCspViolations(page);
  const icons = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "서식", {
    markdown: "첫 문단 글자\n\n둘째 문단\n\n셋째 문단\n",
  });
  await openDoc(page, doc.path);
  const bubble = page.locator("[data-fvoci-bubble]");

  // No bubble without a selection; a selection shows the formatting toolbar.
  await caretAtEndOf(page, 0);
  await expect(bubble).toBeHidden();
  await selectBlockText(page, 0);
  await expect(bubble).toBeVisible();
  const toolbar = bubble.getByRole("toolbar", { name: "서식" });

  // Marks: each button toggles its mark and says whether it is on.
  for (const [name, tag] of [
    ["굵게", "strong"],
    ["기울임", "em"],
    ["밑줄", "u"],
    ["취소선", "s"],
  ] as const) {
    const button = toolbar.getByRole("button", { name, exact: true });
    await expect(button).toHaveAttribute("aria-pressed", "false");
    await button.click();
    await expect(blockAt(page, 0).locator(tag)).toHaveText("첫 문단 글자");
    await expect(button).toHaveAttribute("aria-pressed", "true");
  }
  await toolbar.getByRole("button", { name: "기울임", exact: true }).click();
  await expect(blockAt(page, 0).locator("em")).toHaveCount(0);

  // Block type menu: enters at its first item (paragraph, checked), Escape
  // closes it and returns focus to its trigger, the trigger toggles it.
  const typeTrigger = toolbar.getByRole("button", { name: "본문", exact: true });
  await typeTrigger.click();
  const typeMenu = page.getByRole("menu", { name: "블록 유형" });
  await expect(typeMenu).toBeVisible();
  await expect(typeTrigger).toHaveAttribute("aria-expanded", "true");
  await expect(typeTrigger).toHaveAttribute("aria-controls", (await typeMenu.getAttribute("id"))!);
  const typeItems = typeMenu.getByRole("menuitemradio");
  await expect(typeItems).toHaveText(["본문", "H1", "H2", "H3"]);
  await expect(typeItems.first()).toBeFocused();
  await expect(typeItems.first()).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowDown");
  await expect(typeItems.nth(1)).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(typeMenu).toHaveCount(0);
  await expect(typeTrigger).toBeFocused();
  await typeTrigger.click();
  await expect(typeMenu).toBeVisible();
  await typeTrigger.click();
  await expect(typeMenu).toHaveCount(0);
  await typeTrigger.click();
  await typeMenu.getByRole("menuitemradio", { name: "H2" }).click();
  await expect(typeMenu).toHaveCount(0);
  await expect(editorOf(page).locator(":scope > h2")).toHaveText("첫 문단 글자");
  await expect(toolbar.getByRole("button", { name: "H2", exact: true })).toBeVisible();

  // Link: the field takes focus; Escape closes it and returns to the
  // trigger without touching the selection; Apply links the selection.
  const linkTrigger = toolbar.getByRole("button", { name: "링크", exact: true });
  await linkTrigger.click();
  const linkDialog = page.getByRole("dialog", { name: "링크" });
  await expect(linkDialog.getByLabel("URL")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(linkDialog).toHaveCount(0);
  await expect(linkTrigger).toBeFocused();
  expect(await editorOf(page).evaluate(() => window.getSelection()?.toString())).toBe(
    "첫 문단 글자",
  );
  await linkTrigger.click();
  await page.getByLabel("URL").fill("https://example.com/doc");
  await page.getByRole("button", { name: "적용" }).click();
  await expect(blockAt(page, 0).locator('a[href="https://example.com/doc"]')).toHaveText(
    "첫 문단 글자",
  );

  // Highlight colours.
  await selectBlockText(page, 1);
  const colour = toolbar.getByRole("button", { name: "색", exact: true });
  await expect(colour).toHaveAttribute("aria-pressed", "false");
  await colour.click();
  const colours = page.getByRole("dialog", { name: "형광" });
  await colours.getByRole("button", { name: "노랑" }).click();
  await expect(colours).toHaveCount(0);
  await expect(blockAt(page, 1).locator("mark")).toHaveAttribute("data-color", "var(--accent)");
  await expect(colour).toHaveAttribute("aria-pressed", "true");

  // Lists: a checkbox menu that stays open while items toggle.
  await toolbar.getByRole("button", { name: "목록", exact: true }).click();
  const listMenu = page.getByRole("menu", { name: "목록" });
  await expect(listMenu.getByRole("menuitemcheckbox")).toHaveText(["글머리", "번호", "할 일"]);
  await listMenu.getByRole("menuitemcheckbox", { name: "번호" }).click();
  await expect(editorOf(page).locator(":scope > ol")).toHaveText("둘째 문단");
  await expect(listMenu.getByRole("menuitemcheckbox", { name: "번호" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await page.keyboard.press("Escape");
  await expect(listMenu).toHaveCount(0);

  // Reproduce the focus-repair task arriving after native Shift+Home but before
  // the browser delivers selectionchange. Keep the actual PM callback and native
  // keyboard input; only control when that pending task runs.
  await page.evaluate(() => {
    const root = document.querySelector(".fvoci-editor .ProseMirror") as HTMLElement & {
      editor: {
        state: {
          selection: { from: number; to: number; $from: { parent: { textContent: string } } };
        };
      };
    };
    const snapshot = () => {
      const native = document.getSelection();
      const pm = root.editor.state.selection;
      return {
        text: native?.toString(),
        from: pm.from,
        to: pm.to,
        parent: pm.$from.parent.textContent,
      };
    };
    const gate = { captured: false, delivered: false, before: snapshot(), after: snapshot() };
    Object.assign(window, { __fvociKeyboardFocusRepair: gate });
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
      // Installed prosemirror-view schedules its selection-to-DOM focus repair
      // synchronously in the root focus handler with a 20ms delay.
      if (focusing && delay === 20 && typeof callback === "function") {
        if (repair) throw new Error("multiple editor focus-repair tasks");
        gate.captured = true;
        repair = () => callback(...args);
        timer = nativeTimeout(() => {}, delay);
        return timer;
      }
      return nativeTimeout(callback, delay, ...args);
    }) as typeof window.setTimeout;
    const deliver = (event: KeyboardEvent) => {
      if (event.key !== "Home" || !event.shiftKey) return;
      window.setTimeout = nativeTimeout;
      document.removeEventListener("keyup", deliver);
      window.clearTimeout(timer);
      if (!repair) throw new Error("missing pending editor focus repair");
      gate.before = snapshot();
      repair();
      gate.delivered = true;
      gate.after = snapshot();
    };
    // Bubble phase: the editor's supported keyup handler has completed, while
    // the native selectionchange task has not yet run.
    document.addEventListener("keyup", deliver);
  });
  // The "⋮" menu: alignment (radio items that stay open) and clear formatting.
  await selectBlockText(page, 2);
  const focusRepair = await page.evaluate(
    () =>
      (
        window as unknown as {
          __fvociKeyboardFocusRepair: {
            captured: boolean;
            delivered: boolean;
            before: { text?: string; from: number; to: number; parent: string };
            after: { text?: string; from: number; to: number; parent: string };
          };
        }
      ).__fvociKeyboardFocusRepair,
  );
  await test
    .info()
    .attach("keyboard-focus-repair", {
      body: JSON.stringify(focusRepair),
      contentType: "application/json",
    });
  expect(focusRepair.captured).toBe(true);
  expect(focusRepair.delivered).toBe(true);
  for (const boundary of [focusRepair.before, focusRepair.after]) {
    expect(boundary.text).toBe("셋째 문단");
    expect(boundary.parent).toBe("셋째 문단");
    expect(boundary.to - boundary.from).toBe("셋째 문단".length);
  }
  const more = toolbar.getByRole("button", { name: "서식", exact: true });
  await more.click();
  const moreMenu = page.getByRole("menu", { name: "서식" });
  const aligns = moreMenu.getByRole("menuitemradio");
  await expect(aligns).toHaveText(["왼쪽", "가운데", "오른쪽"]);
  await expect(aligns.first()).toBeFocused();
  await moreMenu.getByRole("menuitemradio", { name: "가운데" }).click();
  await expect(blockAt(page, 2)).toHaveCSS("text-align", "center");
  await expect(moreMenu.getByRole("menuitemradio", { name: "가운데" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
  // Tab leaves the menu from its trigger.
  await moreMenu.getByRole("menuitemradio", { name: "가운데" }).focus();
  await page.keyboard.press("Tab");
  await expect(moreMenu).toHaveCount(0);
  expect(await focused(page)).not.toMatch(/^menuitem/);
  // Clear formatting removes the marks of the selection.
  await toolbar.getByRole("button", { name: "굵게", exact: true }).click();
  await expect(blockAt(page, 2).locator("strong")).toHaveText("셋째 문단");
  await more.click();
  await moreMenu.getByRole("menuitem", { name: "클리어" }).click();
  await expect(moreMenu).toHaveCount(0);
  await expect(blockAt(page, 2).locator("strong")).toHaveCount(0);

  await save(page);
  const saved = JSON.stringify(await savedBody(page.request, wsId, doc.id));
  expect(saved).toContain('"level":2');
  expect(saved).toContain('"href":"https://example.com/doc"');
  expect(saved).toContain('"type":"highlight"');
  expect(saved).toContain('"color":"var(--accent)"');
  expect(saved).toContain('"type":"orderedList"');
  expect(saved).toContain('"textAlign":"center"');
  for (const mark of ["bold", "underline", "strike"]) expect(saved).toContain(`"type":"${mark}"`);
  expect(csp).toEqual([]);
  expect(icons).toEqual([]);
});

test("a cell selection shows the bubble, and bold applies to every selected cell", async ({
  page,
}) => {
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "셀 서식", {
    json: {
      type: "doc",
      content: [
        {
          type: "table",
          content: [
            {
              type: "tableRow",
              content: [tableCell("tableCell", "가"), tableCell("tableCell", "나")],
            },
          ],
        },
        paragraph(""),
      ],
    },
  });
  await openDoc(page, doc.path);
  await editorOf(page).locator("td").first().click();
  await editorOf(page)
    .locator("td")
    .nth(1)
    .click({ modifiers: ["Shift"] });
  await expect(editorOf(page).locator(".selectedCell")).toHaveCount(2);
  const bubble = page.locator("[data-fvoci-bubble]");
  await expect(bubble).toBeVisible();
  await bubble.getByRole("button", { name: "굵게" }).click();
  await expect(editorOf(page).locator("td strong")).toHaveText(["가", "나"]);
});

test("on a narrow screen the gutter and bubble give way to the bottom toolbar", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const csp = watchCspViolations(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "모바일", { markdown: "모바일 문단\n" });
  await openDoc(page, doc.path);

  await blockAt(page, 0).hover();
  await expect(page.locator('[data-gutter="plus"]')).toBeHidden();
  const bar = page.locator("[data-mobile-toolbar]");
  await expect(bar).toBeVisible();
  expect(await bar.evaluate((el) => el.getBoundingClientRect().height)).toBeLessThan(90);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);

  // A selection shows no bubble; the bottom toolbar formats it.
  await selectBlockText(page, 0);
  await expect(page.locator("[data-fvoci-bubble]")).toBeHidden();
  await bar.getByRole("button", { name: "굵게" }).click();
  await expect(blockAt(page, 0).locator("strong")).toHaveText("모바일 문단");

  // Its menus open upwards, above the bar.
  await bar.getByRole("button", { name: "본문", exact: true }).click();
  const typeMenu = page.getByRole("menu", { name: "블록 유형" });
  await expect(typeMenu).toBeVisible();
  await expect(typeMenu).toHaveAttribute("data-side", "top");
  await page.keyboard.press("Escape");

  // "+" inserts a block below through the slash menu.
  await caretAtEndOf(page, 0);
  await bar.getByRole("button", { name: "삽입" }).click();
  await expect(page.locator(".fvoci-suggestion")).toBeVisible();
  await expectBlocks(page, ["모바일 문단", "/"]);
  await page.keyboard.press("Escape");

  // A touch held on the body shows the gutter.
  await editorOf(page).dispatchEvent("pointerdown", { pointerType: "touch", isPrimary: true });
  await expect(page.locator(".fvoci-gutter")).toHaveClass(/fvoci-gutter-touch/);
  await page.dispatchEvent("body", "pointerup", { pointerType: "touch", isPrimary: true });
  await expect(page.locator(".fvoci-gutter")).not.toHaveClass(/fvoci-gutter-touch/);

  await save(page);
  await page.reload();
  await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15_000 });
  await expect(blockAt(page, 0).locator("strong")).toHaveText("모바일 문단");

  // Wide again: no bottom toolbar.
  await page.setViewportSize({ width: 1280, height: 720 });
  await expect(bar).toBeHidden();
  expect(csp).toEqual([]);
});
