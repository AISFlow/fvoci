import { expect, type Page, test, type WebSocketRoute } from "@playwright/test";
import { createEncoder, toUint8Array, writeVarString, writeVarUint } from "lib0/encoding";
import { decodeHocuspocusFrame, frameBytes, persistParts } from "../e2e-pending/collab-wire";
import type { HocuspocusProvider } from "@hocuspocus/provider";
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
  type RawWitness = { doc: Y.Doc; provider: unknown; repairs: number; updates: number };
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
          updates: 0,
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
          witness.updates++;
          if (origin === syncKey) witness.repairs++;
        });
        (window as RawWindow).w3RawWitness = witness;
      });
    }
    await selectMode(page, "markdown");
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await field.fill("원본과 별개의 취소할 초안 🧑‍💻");
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
    await expect(field).toHaveValue("원본과 별개의 취소할 초안 🧑‍💻");
    const beforeCancel = await page.evaluate(() => {
      const witness = (window as RawWindow).w3RawWitness;
      if (!witness) throw new Error("Missing raw document witness");
      return { raw: witness.doc.getXmlFragment("prosemirror").toJSON(), updates: witness.updates };
    });
    // The actual visible button must remain useful after Editor.destroy().
    // Composition blocks deliberate discard even in the raw-readonly panel.
    await field.dispatchEvent("compositionstart");
    await expect(page.getByRole("button", { name: "취소 · 최신 내용 열기" })).toBeDisabled();
    await expect(field).toHaveValue("원본과 별개의 취소할 초안 🧑‍💻");
    await field.dispatchEvent("compositionend");
    await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await expect(field).toHaveValue("");
    await expect(page.getByRole("status").filter({ hasText: "문서가 변경되었습니다" })).toHaveCount(
      0,
    );
    expect(
      await page.evaluate(() => {
        const witness = (window as RawWindow).w3RawWitness;
        if (!witness) throw new Error("Missing raw document witness");
        return {
          raw: witness.doc.getXmlFragment("prosemirror").toJSON(),
          updates: witness.updates,
        };
      }),
    ).toEqual(beforeCancel);
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

type AckWindow = Window & { w3AckSeen?: string[] };
async function observeActualAckDelivery(page: Page): Promise<void> {
  await editorOf(page).evaluate((root) => {
    const editor = (root as EditorElement).editor;
    const extension = editor.extensionManager.extensions.find(
      (item) => item.name === "collaborationCaret",
    );
    if (!extension) throw new Error("Missing actual collaboration provider");
    const options = extension.options as Record<string, unknown>;
    const provider = options.provider as HocuspocusProvider;
    const seen: string[] = [];
    (window as AckWindow).w3AckSeen = seen;
    provider.on("stateless", ({ payload }: { payload: string }) => {
      seen.push(payload);
    });
  });
}
async function expectAckDelivered(page: Page, payload: string): Promise<void> {
  await expect
    .poll(() =>
      page.evaluate((value) => (window as AckWindow).w3AckSeen?.includes(value) ?? false, payload),
    )
    .toBe(true);
}

// Real Rust traffic is forwarded unchanged; only the browser-facing ACK is
// gated to verify exact prefix/lifetime barriers using the installed codec.
async function sourceAckGate(page: Page) {
  const held: {
    id: string;
    routingKey: string;
    message: string | Buffer;
    socket: WebSocketRoute;
  }[] = [];
  let hold = true;
  await page.routeWebSocket(/\/collab(?:\?|$)/, (socket) => {
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      server.send(message);
    });
    server.onMessage((message) => {
      const frame = decodeHocuspocusFrame(frameBytes(message));
      const parts = frame?.kind === "stateless" ? persistParts(frame.payload) : null;
      if (hold && parts?.kind === "done" && frame?.kind === "stateless")
        held.push({ id: parts.id, routingKey: frame.routingKey, message, socket });
      else socket.send(message);
    });
  });
  const stateless = (index: number, payload: string) => {
    const item = held[index];
    if (!item) throw new Error("Missing real held ACK");
    const encoder = createEncoder();
    writeVarString(encoder, item.routingKey);
    writeVarUint(encoder, 5);
    writeVarString(encoder, payload);
    item.socket.send(Buffer.from(toUint8Array(encoder)));
  };
  return {
    held,
    wrong: (index: number) => {
      stateless(index, "persisted:00000000-0000-4000-8000-000000000001");
    },
    fail: (index: number) => {
      const item = held[index];
      if (!item) throw new Error("Missing ACK");
      stateless(index, `persist-failed:${item.id}`);
    },
    release: (index: number) => {
      const item = held[index];
      if (!item) throw new Error("Missing ACK");
      item.socket.send(item.message);
    },
    releaseAll: () => {
      hold = false;
      for (const item of held.splice(0)) item.socket.send(item.message);
    },
  };
}

test("wrong/old-prefix ACK never copies or marks newest source edit saved, matched ACK copies actual live body and a new client sees it", async ({
  page,
  browser,
  baseURL,
}) => {
  const gate = await sourceAckGate(page);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "Markdown ACK 최신 본문", {
    json: {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "ack-body" }, content: [{ type: "text", text: "원본" }] },
      ],
    },
  });
  await openDoc(page, doc.path);
  await observeActualAckDelivery(page);
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.fill("첫 수정");
  await page.getByRole("button", { name: "적용", exact: true }).click();
  await page.evaluate(() => navigator.clipboard.writeText("sentinel"));
  await page.getByRole("button", { name: "저장된 현재 문서 복사" }).click();
  await expect.poll(() => gate.held.length).toBeGreaterThan(0);
  const old = gate.held.length - 1;
  gate.wrong(old);
  await expectAckDelivered(page, "persisted:00000000-0000-4000-8000-000000000001");
  await expect(page.locator('[data-collab-persisted="false"]')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("sentinel");
  await field.fill("가장 최신 수정 🧑‍💻");
  await page.getByRole("button", { name: "적용", exact: true }).click();
  gate.release(old);
  await expect(
    page.getByRole("alert").filter({ hasText: "저장 확인 중 문서가 변경되었습니다" }),
  ).toBeVisible();
  await expect(page.locator('[data-collab-persisted="false"]')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("sentinel");
  const before = gate.held.length;
  await page.getByRole("button", { name: "저장된 현재 문서 복사" }).click();
  await expect.poll(() => gate.held.length).toBeGreaterThan(before);
  gate.release(gate.held.length - 1);
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toContain("가장 최신 수정 🧑‍💻");
  await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible();
  const body = await savedBody(page.request, ws, doc.id);
  expect(body.content?.[0]?.attrs?.id).toBe("ack-body");
  expect(body.content?.[0]?.content?.[0]?.text).toBe("가장 최신 수정 🧑‍💻");
  const fresh = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(fresh.page, doc.path);
    await expectBlocks(fresh.page, ["가장 최신 수정 🧑‍💻"]);
    expect(await savedBody(fresh.page.request, ws, doc.id)).toEqual(body);
  } finally {
    await fresh.context.close();
    gate.releaseAll();
  }
});

test("failed ACK cannot copy source or claim saved even after the server wrote the requested body", async ({
  page,
}) => {
  const gate = await sourceAckGate(page);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "Markdown 저장 실패", { markdown: "원본" });
  await openDoc(page, doc.path);
  await observeActualAckDelivery(page);
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.fill("실패 확인");
  await page.getByRole("button", { name: "적용", exact: true }).click();
  await page.evaluate(() => navigator.clipboard.writeText("failure sentinel"));
  await page.getByRole("button", { name: "저장된 현재 문서 복사" }).click();
  await expect.poll(() => gate.held.length).toBeGreaterThan(0);
  const failed = gate.held.at(-1);
  if (!failed) throw new Error("Missing failed ACK witness");
  gate.fail(gate.held.length - 1);
  await expectAckDelivered(page, `persist-failed:${failed.id}`);
  await expect(
    page.getByRole("alert").filter({ hasText: "저장 확인 중 문서가 변경되었습니다" }),
  ).toBeVisible();
  await expect(page.locator('[data-collab-persisted="false"]')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("failure sentinel");
  gate.releaseAll();
});

test("block arrangement uses live selection and keeps the actual document/provider plus durable IDs", async ({
  page,
}) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "블록 순서", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "first" },
          content: [{ type: "text", text: "한글 첫 블록" }],
        },
        {
          type: "paragraph",
          attrs: { id: "second" },
          content: [{ type: "text", text: "둘째 블록" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await recordIdentity(page);
  await caretAtEndOf(page, 0);
  await selectMode(page, "block");
  await page.getByRole("button", { name: "선택 블록 아래로" }).click();
  await expectBlocks(page, ["둘째 블록", "한글 첫 블록"]);
  await expectIdentity(page);
  await save(page);
  const body = await savedBody(page.request, ws, doc.id);
  expect(body.content?.[0]?.attrs?.id).toBe("second");
  expect(body.content?.[1]?.attrs?.id).toBe("first");
});

test("native backward selection and stored marks survive no-op modes without Y writes or rich keyboard interception", async ({
  page,
}) => {
  await login(page, admin.email, admin.password);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "뒤로 선택", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "selection" },
          content: [{ type: "text", text: "한글 연구 자료" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await recordIdentity(page);
  await caretAtEndOf(page, 0);
  await page.keyboard.press("Control+Shift+ArrowLeft");
  const before = await editorOf(page).evaluate((root) => {
    const state = (root as EditorElement).editor.state;
    return { anchor: state.selection.anchor, head: state.selection.head };
  });
  expect(before.anchor).toBeGreaterThan(before.head);
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.focus();
  await page.keyboard.press("Control+a");
  expect(
    await field.evaluate(
      (element) =>
        (element as HTMLTextAreaElement).selectionEnd -
        (element as HTMLTextAreaElement).selectionStart,
    ),
  ).toBe((await field.inputValue()).length);
  await selectMode(page, "preview");
  await selectMode(page, "rich");
  const after = await editorOf(page).evaluate((root) => {
    const state = (root as EditorElement).editor.state;
    return {
      anchor: state.selection.anchor,
      head: state.selection.head,
      focused: state.selection.empty
        ? false
        : Boolean(document.activeElement?.closest(".ProseMirror")),
    };
  });
  expect({ anchor: after.anchor, head: after.head }).toEqual(before);
  expect(after.focused).toBe(true);
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Control+b");
  const marks = await editorOf(page).evaluate((root) =>
    (root as EditorElement).editor.state.storedMarks?.map((mark) => mark.type.name),
  );
  expect(marks).toContain("bold");
  await selectMode(page, "markdown");
  await selectMode(page, "rich");
  expect(
    await editorOf(page).evaluate((root) =>
      (root as EditorElement).editor.state.storedMarks?.map((mark) => mark.type.name),
    ),
  ).toEqual(marks);
  await expectIdentity(page, 0);
});
