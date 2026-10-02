import { expect, type Page, test } from "@playwright/test";
import type { Editor } from "@tiptap/core";
import type * as Y from "yjs";
import { login } from "./helpers";
import {
  admin,
  blockAt,
  caretAtEndOf,
  createDoc,
  editorOf,
  expectBlocks,
  newSignedInPage,
  openDoc,
  save,
  savedBody,
  setupInstance,
  workspaceId,
} from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
});

async function selectMode(
  page: Page,
  mode: "rich" | "block" | "markdown" | "preview",
): Promise<void> {
  await page.locator(`[data-editor-mode="${mode}"]`).click();
  await expect(page.locator(".fvoci-editor")).toHaveAttribute("data-editor-mode-active", mode);
}

type Witness = {
  editor: Editor;
  doc: Y.Doc;
  provider: unknown;
  fragment: Y.XmlFragment;
  updates: number;
};
type EditorElement = HTMLElement & { editor: Editor; w3Witness?: Witness };

async function recordIdentity(page: Page): Promise<void> {
  await editorOf(page).evaluate((root) => {
    const element = root as EditorElement;
    const editor = element.editor;
    const options = (name: string): Record<string, unknown> => {
      const extension = editor.extensionManager.extensions.find((item) => item.name === name);
      if (!extension) throw new Error(`Missing actual ${name} extension`);
      return extension.options as Record<string, unknown>;
    };
    const doc = options("collaboration").document as Y.Doc;
    const witness: Witness = {
      editor,
      doc,
      provider: options("collaborationCaret").provider,
      fragment: doc.getXmlFragment("prosemirror"),
      updates: 0,
    };
    doc.on("update", () => {
      witness.updates++;
    });
    element.w3Witness = witness;
  });
}

async function expectIdentity(page: Page, updates?: number): Promise<void> {
  const result = await editorOf(page).evaluate((root) => {
    const element = root as EditorElement;
    const witness = element.w3Witness;
    if (!witness) throw new Error("Original editor DOM/witness was replaced");
    const options = (name: string) =>
      element.editor.extensionManager.extensions.find((item) => item.name === name)?.options as
        Record<string, unknown> | undefined;
    return {
      sameEditor: element.editor === witness.editor,
      sameDoc: options("collaboration")?.document === witness.doc,
      sameProvider: options("collaborationCaret")?.provider === witness.provider,
      sameFragment: witness.doc.getXmlFragment("prosemirror") === witness.fragment,
      updates: witness.updates,
    };
  });
  expect(result.sameEditor && result.sameDoc && result.sameProvider && result.sameFragment).toBe(
    true,
  );
  if (updates !== undefined) expect(result.updates).toBe(updates);
}

test("four modes keep the same actual editor/doc/provider/fragment and no-op/Cancel publish zero content updates", async ({
  page,
}) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "네 모드 한글 🧑‍💻", {
    json: {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { id: "heading", level: 2 },
          content: [{ type: "text", text: "한글 연구 🧑‍💻" }],
        },
        { type: "paragraph", attrs: { id: "body" }, content: [{ type: "text", text: "자료" }] },
        { type: "paragraph", attrs: { id: "empty" } },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const body = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await selectMode(page, "block");
  await expect(page.getByRole("button", { name: "선택 블록 아래로" })).toBeVisible();
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await expect(field).toBeEditable();
  const source = await field.inputValue();
  await field.fill(`${source}\n\n취소할 초안`);
  await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
  await expect(field).toHaveValue(source);
  await selectMode(page, "preview");
  await expect(page.locator(".fvoci-mode-preview")).toContainText("한글 연구 🧑‍💻");
  await selectMode(page, "rich");
  await expectIdentity(page, 0);
  expect(await savedBody(page.request, ws, doc.id)).toEqual(body);
});

test("source edit, actual peer edit and same-actor undo retain IDs and newest durable body in a genuinely new client", async ({
  browser,
  baseURL,
  page,
}) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "Markdown 동료 저장", {
    json: {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "p" }, content: [{ type: "text", text: "한글 연구" }] },
        { type: "paragraph", attrs: { id: "other" }, content: [{ type: "text", text: "자료" }] },
      ],
    },
  });
  const peer = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(page, doc.path);
    await openDoc(peer.page, doc.path);
    await recordIdentity(page);
    await selectMode(page, "markdown");
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await field.fill((await field.inputValue()).replace("연구", "조사"));
    await page.getByRole("button", { name: "적용", exact: true }).click();
    await expectBlocks(peer.page, ["한글 조사", "자료"]);
    await caretAtEndOf(peer.page, 0);
    await peer.page.keyboard.type(" 동료");
    await selectMode(page, "rich");
    await expectBlocks(page, ["한글 조사 동료", "자료"]);
    await blockAt(page, 0).click();
    await page.keyboard.press("Control+z");
    await expectBlocks(page, ["한글 연구 동료", "자료"]);
    await expectBlocks(peer.page, ["한글 연구 동료", "자료"]);
    await expectIdentity(page);
    await save(page);
    const body = await savedBody(page.request, ws, doc.id);
    expect(body.content?.[0]?.attrs?.id).toBe("p");
    expect(body.content?.[0]?.content?.[0]?.text).toBe("한글 연구 동료");
    expect(body.content?.[1]?.attrs?.id).toBe("other");
    const fresh = await newSignedInPage(browser, baseURL, admin);
    try {
      await openDoc(fresh.page, doc.path);
      await expectBlocks(fresh.page, ["한글 연구 동료", "자료"]);
      expect(await savedBody(fresh.page.request, ws, doc.id)).toEqual(body);
    } finally {
      await fresh.context.close();
    }
  } finally {
    await peer.context.close();
  }
});

test("dirty source refuses delete-only peer changes and Cancel keeps current peer state", async ({
  browser,
  baseURL,
  page,
}) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "동료 삭제 경합", { markdown: "연구\n\n자료" });
  const peer = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(page, doc.path);
    await openDoc(peer.page, doc.path);
    await selectMode(page, "markdown");
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    const dirty = (await field.inputValue()).replace("연구", "조사");
    await field.fill(dirty);
    await caretAtEndOf(peer.page, 1);
    await peer.page.keyboard.press("Backspace");
    await expect(
      page.getByRole("status").filter({ hasText: "문서가 변경되었습니다" }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "적용", exact: true })).toBeDisabled();
    await expect(field).toHaveValue(dirty);
    await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await selectMode(page, "rich");
    await expectBlocks(page, ["연구", "자"]);
  } finally {
    await peer.context.close();
  }
});

test("composition/keyCode229, narrow reflow and enlarged CJK source retain input and mode identity", async ({
  page,
}, testInfo) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(
    page.request,
    ws,
    "긴 한글 제목과 Markdown 직접 편집 화면의 확대·키보드 흐름",
    {
      markdown: "한글 연구 🧑‍💻\n\n긴 주소 https://example.com/abcdefghijklmnopqrstuvwxyz0123456789",
    },
  );
  await openDoc(page, doc.path);
  await recordIdentity(page);
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.dispatchEvent("compositionstart", { data: "한" });
  await field.dispatchEvent("keydown", { key: "Process", keyCode: 229, isComposing: true });
  await expect(page.locator('[data-editor-mode="rich"]')).toBeDisabled();
  await field.dispatchEvent("compositionend", { data: "한글" });
  await expect(page.locator('[data-editor-mode="rich"]')).toBeEnabled();
  await field.focus();
  await page.keyboard.press("Control+a");
  const selected = await field.evaluate((element) => {
    if (!(element instanceof HTMLTextAreaElement))
      throw new Error("Expected native source textarea");
    return element.selectionEnd - element.selectionStart;
  });
  expect(selected).toBe((await field.inputValue()).length);
  for (const [name, width, font] of [
    ["desktop", 1280, "100%"],
    ["narrow320", 320, "100%"],
    ["text200", 640, "200%"],
    ["app18", 640, "18px"],
    ["app20", 640, "20px"],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate((size) => {
      document.documentElement.style.fontSize = size;
    }, font);
    await expect(field).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`w3-${name}.png`), fullPage: true });
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    expect(overflow).toBe(false);
  }
  await expectIdentity(page, 0);
});

test("late older-schema peer data retires only the editor, preserving the same live raw document and supported edit through save/re-entry", async ({
  browser,
  baseURL,
  page,
}) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "미래 데이터 보존", {
    json: {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "p" }, content: [{ type: "text", text: "원본" }] },
        { type: "paragraph", attrs: { id: "other" }, content: [{ type: "text", text: "동료" }] },
      ],
    },
  });
  const peer = await newSignedInPage(browser, baseURL, admin);
  type RawWitness = { doc: Y.Doc; provider: unknown; repairs: number };
  type RawWindow = Window & { w3RawWitness?: RawWitness };
  try {
    await openDoc(page, doc.path);
    await openDoc(peer.page, doc.path);
    for (const target of [page, peer.page]) {
      await editorOf(target).evaluate((root) => {
        const editor = (root as EditorElement).editor;
        const options = (name: string) =>
          editor.extensionManager.extensions.find((item) => item.name === name)?.options as
            Record<string, unknown> | undefined;
        const doc = options("collaboration")?.document as Y.Doc;
        const witness: RawWitness = {
          doc,
          provider: options("collaborationCaret")?.provider,
          repairs: 0,
        };
        const syncKey = editor.state.plugins
          .map((plugin) => plugin.spec.key)
          .find((key) => {
            const value: unknown = key?.getState(editor.state);
            return (
              typeof value === "object" && value !== null && "doc" in value && value.doc === doc
            );
          });
        if (!syncKey) throw new Error("Missing actual ySync key");
        doc.on("update", (_update: Uint8Array, origin: unknown) => {
          if (origin === syncKey) witness.repairs++;
        });
        (window as RawWindow).w3RawWitness = witness;
      });
    }
    await selectMode(page, "markdown");
    await peer.page.evaluate(() => {
      const witness = (window as RawWindow).w3RawWitness;
      if (!witness) throw new Error("Missing raw document witness");
      const { doc } = witness;
      doc.transact(() => {
        const fragment = doc.getXmlFragment("prosemirror");
        const existing = fragment.get(0) as Y.XmlElement;
        const future = existing.clone();
        future.nodeName = "futureNode";
        future.setAttribute("id", "future-preserved");
        fragment.insert(1, [future]);
        const supported = fragment.get(2) as Y.XmlElement;
        const text = supported.get(0) as Y.XmlText;
        text.insert(text.length, " 동시 변경");
      }, "older-schema-fixture");
    });
    await expect(page.getByRole("alert").filter({ hasText: "future-preserved" })).toBeVisible();
    await expect(
      peer.page.getByRole("alert").filter({ hasText: "future-preserved" }),
    ).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Markdown 직접 편집" })).not.toBeEditable();
    await expect(page.getByRole("button", { name: "적용", exact: true })).toBeDisabled();
    for (const target of [page, peer.page]) {
      const current = await target.evaluate(() => {
        const witness = (window as RawWindow).w3RawWitness;
        if (!witness) throw new Error("Missing raw document witness");
        return {
          repairs: witness.repairs,
          raw: witness.doc.getXmlFragment("prosemirror").toJSON(),
          destroyed: witness.doc.isDestroyed,
        };
      });
      expect(current.repairs).toBe(0);
      expect(current.raw).toContain("futureNode");
      expect(current.raw).toContain('id="future-preserved"');
      expect(current.raw).toContain("동료 동시 변경");
      expect(current.destroyed).toBe(false);
    }
    await save(page);
    const body = await savedBody(page.request, ws, doc.id);
    expect(body.content?.[1]?.type).toBe("futureNode");
    expect(body.content?.[1]?.attrs?.id).toBe("future-preserved");
    expect(body.content?.[2]?.content?.[0]?.text).toBe("동료 동시 변경");
    const fresh = await newSignedInPage(browser, baseURL, admin);
    try {
      await fresh.page.goto(doc.path);
      await expect(
        fresh.page.getByRole("alert").filter({ hasText: "future-preserved" }),
      ).toBeVisible();
      expect(await savedBody(fresh.page.request, ws, doc.id)).toEqual(body);
    } finally {
      await fresh.context.close();
    }
  } finally {
    await peer.context.close();
  }
});
