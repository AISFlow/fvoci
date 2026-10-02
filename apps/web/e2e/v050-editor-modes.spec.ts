import {
  expect,
  type Page,
  type WebSocketRoute,
  test as baseTest,
  type BrowserContext,
} from "@playwright/test";
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { createEncoder, toUint8Array, writeVarString, writeVarUint } from "lib0/encoding";
import { decodeHocuspocusFrame, frameBytes, persistParts } from "../e2e-pending/collab-wire";
import type { HocuspocusProvider } from "@hocuspocus/provider";
import type { Editor } from "@tiptap/core";
import "@tiptap/extension-table";
import type * as Y from "yjs";
import { flowSchemas, readJson } from "./helpers";
import {
  admin,
  blockAt,
  caretAtEndOf,
  createDoc,
  editorOf,
  expectBlocks,
  newSignedInPage as passwordSignedInPage,
  openDoc,
  save,
  savedBody,
  setupInstance,
  workspaceId,
} from "./workspace-wiki-vue-editor";

type SessionState = Awaited<ReturnType<BrowserContext["storageState"]>>;
let fixtureSession: SessionState | undefined;
let logoutSession: SessionState | undefined;
const logoutCase =
  "real router navigation protects a private Markdown draft and actual logout retires it without saving";

// Each client retains its own browser/Y.Doc/socket; genuine password sign-ins
// supply session cookies once rather than exhausting the real auth budget.
const test = baseTest.extend({
  storageState: async ({ baseURL }, use, testInfo) => {
    const session = testInfo.title === logoutCase ? logoutSession : fixtureSession;
    if (!baseURL || !session) throw new Error("Missing genuine fixture session");
    await use(session);
  },
});
test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
  const primary = await passwordSignedInPage(browser, baseURL, admin);
  try {
    fixtureSession = await primary.context.storageState();
  } finally {
    await primary.context.close();
  }
  const logout = await passwordSignedInPage(browser, baseURL, admin);
  try {
    logoutSession = await logout.context.storageState();
  } finally {
    await logout.context.close();
  }
});

async function authenticatedHome(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
}

async function newSignedInPage(
  ...[browser, baseURL, who, options = {}]: Parameters<typeof passwordSignedInPage>
): ReturnType<typeof passwordSignedInPage> {
  expect(who.email).toBe(admin.email);
  if (!fixtureSession) throw new Error("Missing genuine fixture session");
  const context = await browser.newContext({
    baseURL,
    storageState: fixtureSession,
    permissions: options.permissions ?? [],
  });
  try {
    const page = await context.newPage();
    await authenticatedHome(page);
    return { context, page };
  } catch (error) {
    await context.close();
    throw error;
  }
}

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

/** Read only through this invocation's actual restricted app role. */
function restrictedDbBody(workspace: string, document: string): unknown {
  const container = process.env.FVOCI_TEST_PG_CONTAINER;
  const connection = process.env.DATABASE_APP_URL;
  if (!container?.startsWith("fvoci-rust-test-pg-") || !connection)
    throw new Error("Missing owned isolated PostgreSQL app-role fixture");
  const app = new URL(connection);
  if (
    app.hostname !== "127.0.0.1" ||
    !/^fvoci_app_fvoci_e2e_[a-f0-9]{16}$/.test(app.username) ||
    !/^\/fvoci_e2e_[a-f0-9]{16}$/.test(app.pathname)
  )
    throw new Error("Refusing a non-fixture DB connection");
  for (const id of [workspace, document])
    if (!/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(id))
      throw new Error("Invalid fixture identity");
  const result = spawnSync(
    "docker",
    [
      "exec",
      "-i",
      container,
      "psql",
      "-X",
      "-qAt",
      "-U",
      app.username,
      "-d",
      app.pathname.slice(1),
      "-v",
      "ON_ERROR_STOP=1",
    ],
    {
      input: `BEGIN READ ONLY;
SET LOCAL app.tenant_id = '${workspace}';
SELECT jsonb_build_object('role', current_user, 'superuser', r.rolsuper,
 'bypassRls', r.rolbypassrls, 'tenant', public.app_tenant_id(),
 'rls', c.relrowsecurity, 'forced', c.relforcerowsecurity,
 'notOwner', pg_get_userbyid(c.relowner) <> current_user,
 'rlsActive', row_security_active(c.oid),
 'content', d.content_json, 'version', d.version)
FROM pg_roles r JOIN pg_class c ON c.oid = 'fvoci.documents'::regclass
JOIN fvoci.documents d ON d.id = '${document}' AND d.workspace_id = '${workspace}'
WHERE r.rolname = current_user;
ROLLBACK;`,
      encoding: "utf8",
      timeout: 10000,
    },
  );
  expect(result.status, "restricted read-only DB witness exit").toBe(0);
  const witness: unknown = JSON.parse(result.stdout);
  expect(witness).toMatchObject({
    role: app.username,
    superuser: false,
    bypassRls: false,
    tenant: workspace,
    rls: true,
    rlsActive: true,
    notOwner: true,
    forced: expect.any(Boolean),
  });
  return witness;
}

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

/** Fixture-only runtime diagnostics; no auth headers, tokens or credential IDs. */
async function readLiveRuntime(page: Page): Promise<unknown> {
  return editorOf(page).evaluate((root) => {
    const element = root as EditorElement;
    const editor = element.editor;
    const options = (name: string) =>
      editor.extensionManager.extensions.find((item) => item.name === name)?.options as
        Record<string, unknown> | undefined;
    const doc = options("collaboration")?.document as Y.Doc;
    const provider = options("collaborationCaret")?.provider as HocuspocusProvider;
    const shell = element.closest(".fvoci-editor") as HTMLElement & {
      __vueParentComponent?: { props?: { modeScope?: unknown; editable?: unknown } };
    };
    const app = document.querySelector("#root") as HTMLElement & {
      __vue_app__: {
        _context: {
          provides: {
            VUE_QUERY_CLIENT: {
              getQueriesData(options: { queryKey: readonly string[] }): [unknown, unknown][];
            };
          };
        };
      };
    };
    const metadata = app.__vue_app__._context.provides.VUE_QUERY_CLIENT.getQueriesData({
      queryKey: ["document"],
    }).map(([key, data]) => ({
      key,
      status:
        typeof data === "object" && data !== null && "status" in data ? data.status : undefined,
    }));
    const scope = shell.__vueParentComponent?.props?.modeScope;
    return {
      editable: editor.isEditable,
      pm: editor.getJSON() as unknown,
      rawXML: doc.getXmlFragment("prosemirror").toJSON(),
      sameDoc: doc === element.w3Witness?.doc,
      sameProvider: provider === element.w3Witness?.provider,
      localUpdates: element.w3Witness?.updates,
      authenticated: provider.isAuthenticated,
      authorizedScope: provider.authorizedScope,
      socketStatus: provider.configuration.websocketProvider.status,
      modeScope: typeof scope === "number" ? scope : undefined,
      editableProp: shell.__vueParentComponent?.props?.editable,
      metadata,
    };
  });
}

test("four modes keep the same actual editor/doc/provider/fragment and no-op/Cancel publish zero content updates", async ({
  page,
}) => {
  await authenticatedHome(page);
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
  await authenticatedHome(page);
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
  await authenticatedHome(page);
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
  await authenticatedHome(page);
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
  await authenticatedHome(page);
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
          nodeNames: witness.doc
            .getXmlFragment("prosemirror")
            .toArray()
            .map((node) => (node as Y.XmlElement).nodeName),
          destroyed: witness.doc.isDestroyed,
        };
      });
      expect(current.repairs).toBe(0);
      expect(current.nodeNames).toEqual(["paragraph", "futureNode", "paragraph"]);
      expect(current.raw).toContain("futurenode");
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
type CspWindow = Window & { w3Csp?: { directive: string; blocked: string }[] };
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
}, testInfo) => {
  const gate = await sourceAckGate(page);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await authenticatedHome(page);
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
  expect(body.content?.[0]?.content).toEqual([
    { type: "text", text: "가장 최신 수정 " },
    { type: "emoji", attrs: { name: "technologist" } },
  ]);
  const sql = restrictedDbBody(ws, doc.id);
  expect(sql).toHaveProperty("content", body);
  await testInfo.attach("w3-matched-ACK-app-role-DB.json", {
    body: JSON.stringify(sql),
    contentType: "application/json",
  });
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
  await authenticatedHome(page);
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
  await authenticatedHome(page);
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
  await authenticatedHome(page);
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

test("fresh authenticated readonly client copies only after a genuine current committed-body response, preserving body and live identity", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "읽기 전용 현재 본문", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "readonly-body" },
          content: [{ type: "text", text: "보관된 한글 🧑‍💻" }],
        },
      ],
    },
  });
  expect(
    (
      await page.request.patch(`/api/v1/workspaces/${ws}/documents/${doc.id}`, {
        data: { status: "archived" },
      })
    ).status(),
  ).toBe(200);
  const before = await savedBody(page.request, ws, doc.id);
  const readonly = await newSignedInPage(browser, baseURL, admin, {
    permissions: ["clipboard-read", "clipboard-write"],
  });
  let requested = false;
  let release: () => void = () => {};
  const delivery = new Promise<void>((done) => {
    release = done;
  });
  let actualBody: unknown;
  const bodyUrl = `**/api/v1/workspaces/${ws}/documents/${doc.id}/body`;
  try {
    await openDoc(readonly.page, doc.path);
    await recordIdentity(readonly.page);
    await readonly.page.route(bodyUrl, async (route) => {
      const response = await route.fetch();
      expect(response.status()).toBe(200);
      actualBody = await response.json();
      requested = true;
      await delivery;
      await route.fulfill({ response });
    });
    await expect(editorOf(readonly.page)).toHaveAttribute("contenteditable", "false");
    await selectMode(readonly.page, "markdown");
    const field = readonly.page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await expect(field).not.toBeEditable();
    await testInfo.attach("w3-H1-live-before-copy.json", {
      body: JSON.stringify({ live: await readLiveRuntime(readonly.page), committed: before }),
      contentType: "application/json",
    });
    await readonly.page.evaluate(() => navigator.clipboard.writeText("readonly sentinel"));
    await readonly.page.getByRole("button", { name: "저장된 현재 문서 복사" }).click();
    await expect.poll(() => requested).toBe(true);
    expect(actualBody).toMatchObject({ contentJson: before });
    expect(await readonly.page.evaluate(() => navigator.clipboard.readText())).toBe(
      "readonly sentinel",
    );
    release();
    try {
      await expect
        .poll(() => readonly.page.evaluate(() => navigator.clipboard.readText()))
        .toContain("보관된 한글 🧑‍💻");
    } finally {
      await testInfo.attach("w3-H1-live-after-response.json", {
        body: JSON.stringify({
          live: await readLiveRuntime(readonly.page),
          actualBody,
          panel: await readonly.page.locator(".fvoci-source-panel").textContent(),
          alerts: await readonly.page.locator(".fvoci-editor [role=alert]").allTextContents(),
        }),
        contentType: "application/json",
      });
    }
    await selectMode(readonly.page, "preview");
    await expect(readonly.page.locator(".fvoci-mode-preview")).toContainText("보관된 한글 🧑‍💻");
    await selectMode(readonly.page, "rich");
    await expectIdentity(readonly.page, 0);
    expect(await savedBody(readonly.page.request, ws, doc.id)).toEqual(before);
    const fresh = await newSignedInPage(browser, baseURL, admin);
    try {
      await openDoc(fresh.page, doc.path);
      await expectBlocks(fresh.page, ["보관된 한글 🧑‍💻"]);
      expect(await savedBody(fresh.page.request, ws, doc.id)).toEqual(before);
    } finally {
      await fresh.context.close();
    }
  } finally {
    release();
    await readonly.context.close();
  }
});

test("actual preview producer and SafeHtml sink keep rich heading, colors, table geometry and reference/file meaning; loss warning Cancel leaves exact live state", async ({
  page,
}, testInfo) => {
  await page.addInitScript(() => {
    (window as CspWindow).w3Csp = [];
    document.addEventListener("securitypolicyviolation", (event) => {
      (window as CspWindow).w3Csp?.push({
        directive: event.effectiveDirective,
        blocked: event.blockedURI,
      });
    });
  });
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const target = await createDoc(page.request, ws, "참조 대상 🧑‍💻");
  const doc = await createDoc(page.request, ws, "미리보기 손실 경고", {
    json: {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { id: "preview-heading", level: 4, textAlign: "right" },
          content: [{ type: "text", text: "한글 제목 🧑‍💻" }],
        },
        {
          type: "paragraph",
          attrs: { id: "mixed-p", textAlign: "center" },
          content: [
            {
              type: "text",
              text: "빨강",
              marks: [{ type: "underline" }, { type: "textStyle", attrs: { color: "#112233" } }],
            },
            {
              type: "text",
              text: " 파랑",
              marks: [{ type: "underline" }, { type: "textStyle", attrs: { color: "#445566" } }],
            },
            {
              type: "text",
              text: " <script>hostile()</script>",
              marks: [{ type: "link", attrs: { href: "javascript:hostile()", target: "_self" } }],
            },
          ],
        },
        {
          type: "table",
          attrs: { id: "preview-table" },
          content: [
            {
              type: "tableRow",
              attrs: { id: "preview-row" },
              content: [
                {
                  type: "tableHeader",
                  attrs: {
                    id: "preview-header",
                    colspan: 1,
                    rowspan: 1,
                    colwidth: [160],
                    background: "#abcdef",
                  },
                  content: [
                    {
                      type: "paragraph",
                      attrs: { id: "preview-cell-p" },
                      content: [{ type: "text", text: "표 제목" }],
                    },
                  ],
                },
                {
                  type: "tableCell",
                  attrs: { id: "preview-cell", colspan: 1, rowspan: 1, colwidth: [200] },
                  content: [
                    {
                      type: "paragraph",
                      attrs: { id: "preview-cell2-p" },
                      content: [{ type: "text", text: "표 내용" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
        {
          type: "callout",
          attrs: { id: "preview-callout", kind: "warning" },
          content: [
            {
              type: "paragraph",
              attrs: { id: "preview-warning-p" },
              content: [{ type: "text", text: "경고 의미" }],
            },
          ],
        },
        {
          type: "codeBlock",
          attrs: { id: "preview-code", language: "typescript", highlightLines: [1] },
          content: [{ type: "text", text: 'const 한글 = "🧑‍💻";' }],
        },
        {
          type: "attachment",
          attrs: {
            id: "10000000-0000-4000-8000-000000000009",
            name: "자료%20연구.pdf",
            caption: "첨부 설명",
            image: false,
            width: 70,
            align: "left",
          },
        },
        { type: "embed", attrs: { id: "preview-ref", entity: "document", ref: target.id } },
        {
          type: "paragraph",
          attrs: { id: "preview-tail" },
          content: [{ type: "text", text: "끝" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await page.evaluate(() => {
    (window as CspWindow).w3Csp = [];
  });
  await selectMode(page, "preview");
  const preview = page.locator(".fvoci-mode-preview");
  await expect(preview.locator("h4")).toHaveText("한글 제목 🧑‍💻");
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            resolve();
          }),
        ),
      ),
  );
  const header = (await page.request.get(doc.path)).headers()["content-security-policy"];
  const computed = await preview.evaluate((root) => {
    const h4 = root.querySelector("h4"),
      underline = root.querySelector("u"),
      cell = root.querySelector("th"),
      column = root.querySelector("col");
    if (!h4 || !underline || !cell || !column)
      throw new Error("Missing actual preview producer nodes");
    return {
      alignment: getComputedStyle(h4).textAlign,
      color: getComputedStyle(underline).color,
      background: getComputedStyle(cell).backgroundColor,
      columnWidth: getComputedStyle(column).width,
      html: root.innerHTML,
      violations: (window as CspWindow).w3Csp,
    };
  });
  await testInfo.attach("w3-preview-served-csp-computed.json", {
    body: JSON.stringify({ header, computed }, null, 2),
    contentType: "application/json",
  });
  expect(header).toContain("style-src");
  expect(header).not.toContain("'unsafe-inline'");
  expect(header).not.toContain("'unsafe-hashes'");
  await expect(preview.locator("h4")).toHaveCSS("text-align", "right");
  await expect(preview.locator("u").first()).toHaveText("빨강");
  await expect(preview.locator("u").first()).toHaveCSS("color", "rgb(17, 34, 51)");
  await expect(preview.locator("th")).toHaveAttribute("colwidth", "160");
  await expect(preview.locator("th")).toHaveCSS("background-color", "rgb(171, 205, 239)");
  await expect(preview.locator("col").first()).toHaveCSS("width", "160px");
  await expect(preview.locator("aside[data-kind='warning']")).toContainText("경고 의미");
  await expect(preview.locator("pre code")).toContainText('const 한글 = "🧑‍💻";');
  await expect(preview.locator(".afn-attachment-name")).toHaveText("자료 연구.pdf");
  await expect(preview.locator("figcaption")).toHaveText("첨부 설명");
  await expect(preview.locator(".afn-attachment")).toHaveAttribute(
    "href",
    new RegExp(
      `/api/v1/workspaces/${ws}/attachments/10000000-0000-4000-8000-000000000009/download`,
    ),
  );
  await expect(preview.locator(".afn-embed-ref")).toHaveText("참조 대상 🧑‍💻");
  await expect(preview.locator(".afn-embed-target")).toHaveText(target.id);
  await expect(preview).toContainText("<script>hostile()</script>");
  expect(await preview.locator("script,[onclick],[onerror],a[href^='javascript:']").count()).toBe(
    0,
  );
  await page.screenshot({ path: testInfo.outputPath("w3-rich-preview.png"), fullPage: true });
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.fill((await field.inputValue()).replace("빨강 파랑", "혼합 형식 전체 교체"));
  await page.getByRole("button", { name: "적용", exact: true }).click();
  const loss = page.getByRole("alert").filter({ hasText: "mixed-p" });
  await expect(loss).toContainText("marks");
  await expect(loss).toContainText("document.content.1");
  await expectIdentity(page, 0);
  await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
  await expect(field).not.toHaveValue(/혼합 형식 전체 교체/);
  await selectMode(page, "rich");
  await expectIdentity(page, 0);
  expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
});

async function refetchActualQuery(page: Page, key: readonly string[]): Promise<void> {
  await page.evaluate(async (queryKey) => {
    const root = document.getElementById("root") as HTMLElement & {
      __vue_app__: {
        _context: {
          provides: {
            VUE_QUERY_CLIENT: {
              refetchQueries(options: { queryKey: readonly string[] }): Promise<void>;
            };
          };
        };
      };
    };
    await root.__vue_app__._context.provides.VUE_QUERY_CLIENT.refetchQueries({ queryKey });
  }, key);
}

test("pending block-math permission notification makes zero local readonly writes and retains current stored content in a fresh client", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "수식 권한 경합", {
    json: {
      type: "doc",
      content: [
        { type: "math", attrs: { id: "math-owned", latex: "x + y" } },
        {
          type: "paragraph",
          attrs: { id: "math-tail" },
          content: [{ type: "text", text: "뒤 문단" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await testInfo.attach("w3-D3-initial-runtime.json", {
    body: JSON.stringify(await readLiveRuntime(page)),
    contentType: "application/json",
  });
  await page.getByTitle("수식 편집", { exact: true }).click();
  const field = page.getByRole("textbox", { name: "수식 LaTeX" });
  await field.fill("private pending latex 🧑‍💻");
  expect(
    (
      await page.request.patch(`/api/v1/workspaces/${ws}/documents/${doc.id}`, {
        data: { status: "archived" },
      })
    ).status(),
  ).toBe(200);
  // Refetch permission through the actual query owner without blurring first.
  await refetchActualQuery(page, ["document"]);
  await expect(editorOf(page)).toHaveAttribute("contenteditable", "false");
  const observed = await editorOf(page).evaluate((root) => {
    const element = root as EditorElement;
    return {
      editable: element.editor.isEditable,
      latex: element.editor.state.doc.child(0).attrs.latex as unknown,
      updates: element.w3Witness?.updates,
    };
  });
  await testInfo.attach("w3-math-readonly-local.json", {
    body: JSON.stringify(observed),
    contentType: "application/json",
  });
  expect(observed).toEqual({ editable: false, latex: "x + y", updates: 0 });
  await testInfo.attach("w3-D3-archived-runtime.json", {
    body: JSON.stringify(await readLiveRuntime(page)),
    contentType: "application/json",
  });
  await expectIdentity(page, 0);
  expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
  const fresh = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(fresh.page, doc.path);
    expect(await savedBody(fresh.page.request, ws, doc.id)).toEqual(before);
    expect(
      await editorOf(fresh.page).evaluate(
        (root) => (root as EditorElement).editor.state.doc.child(0).attrs.latex as unknown,
      ),
    ).toBe("x + y");
  } finally {
    await fresh.context.close();
  }
  expect(
    (
      await page.request.patch(`/api/v1/workspaces/${ws}/documents/${doc.id}`, {
        data: { status: "draft" },
      })
    ).status(),
  ).toBe(200);
  await refetchActualQuery(page, ["document"]);
  try {
    await expect(editorOf(page)).toHaveAttribute("contenteditable", "true");
  } finally {
    await testInfo.attach("w3-D3-regrant-runtime.json", {
      body: JSON.stringify({
        live: await readLiveRuntime(page),
        actualMeta: (await (
          await page.request.get(`/api/v1/workspaces/${ws}/documents/${doc.id}`)
        ).json()) as unknown,
      }),
      contentType: "application/json",
    });
  }
  await page.getByTitle("수식 편집", { exact: true }).click();
  await expect(field).toHaveValue("private pending latex 🧑‍💻");
  await field.fill("z + 1");
  await caretAtEndOf(page, 1);
  await save(page);
  const body = await savedBody(page.request, ws, doc.id);
  expect(body.content?.[0]?.attrs?.latex).toBe("z + 1");
  expect(body.content?.[0]?.attrs?.id).toBe("math-owned");
});

test("focused visible Math Cancel never publishes its draft, and detached old field events cannot consume a newly opened draft", async ({
  page,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "수식 취소 소유권", {
    json: {
      type: "doc",
      content: [
        { type: "math", attrs: { id: "cancel-math", latex: "x + y" } },
        {
          type: "paragraph",
          attrs: { id: "cancel-tail" },
          content: [{ type: "text", text: "문단" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  const field = page.getByRole("textbox", { name: "수식 LaTeX" });
  const cancel = editorOf(page).getByRole("button", { name: "취소 · 최신 내용 열기" });
  await page.getByTitle("수식 편집", { exact: true }).click();
  await field.fill("must discard 🧑‍💻");
  await field.evaluate((element) => {
    (window as Window & { w3OldMathField?: HTMLTextAreaElement }).w3OldMathField =
      element as HTMLTextAreaElement;
  });
  await expect(field).toBeFocused();
  await cancel.click();
  expect(
    await editorOf(page).evaluate(
      (root) => (root as EditorElement).editor.state.doc.child(0).attrs.latex as unknown,
    ),
  ).toBe("x + y");
  await expectIdentity(page, 0);
  expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
  await page.getByTitle("수식 편집", { exact: true }).click();
  await field.fill("current new private");
  await page.evaluate(() => {
    const old = (window as Window & { w3OldMathField?: HTMLTextAreaElement }).w3OldMathField;
    if (!old || old.isConnected) throw new Error("Expected removed old Math textarea");
    old.value = "retired target must not publish";
    old.dispatchEvent(new Event("input", { bubbles: true }));
    old.dispatchEvent(new FocusEvent("blur", { bubbles: true }));
  });
  await expect(field).toHaveValue("current new private");
  await expectIdentity(page, 0);
  await cancel.click();
  await expectIdentity(page, 0);
  for (const activate of ["Space", "Enter"] as const) {
    await page.getByTitle("수식 편집", { exact: true }).click();
    await field.fill(`keyboard ${activate} must discard`);
    await page.keyboard.press("Tab");
    await expect(cancel).toBeFocused();
    await expectIdentity(page, 0);
    await page.keyboard.press("Shift+Tab");
    await expect(field).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(cancel).toBeFocused();
    await page.screenshot({
      path: testInfo.outputPath(`w3-math-keyboard-${activate}.png`),
      fullPage: true,
    });
    await page.keyboard.press(activate);
    await expect(field).toHaveCount(0);
    await expectIdentity(page, 0);
  }
  await page.getByTitle("수식 편집", { exact: true }).click();
  await field.fill("authorized final");
  await expect(field).toBeFocused();
  await expect(field).toHaveValue("authorized final");
  await field.evaluate((element) => {
    element.addEventListener("blur", (event) => {
      const field = element as HTMLTextAreaElement;
      const related = (event as FocusEvent).relatedTarget as HTMLElement | null;
      const log = {
        value: field.value,
        connected: field.isConnected,
        target: related?.tagName,
        role: related?.getAttribute("role"),
        label: related?.getAttribute("aria-label"),
      };
      (window as Window & { w3MathBlur?: unknown }).w3MathBlur = log;
    });
  });
  await caretAtEndOf(page, 1);
  await testInfo.attach("w3-current-math-blur.json", {
    body: JSON.stringify(
      await page.evaluate(() => ({
        blur: (window as Window & { w3MathBlur?: unknown }).w3MathBlur,
        active: document.activeElement?.tagName,
        remaining: document.querySelector('textarea[aria-label="수식 LaTeX"]')?.outerHTML,
        node: (
          document.querySelector(".fvoci-editor .ProseMirror") as EditorElement
        ).editor.state.doc
          .child(0)
          .toJSON() as unknown,
      })),
    ),
    contentType: "application/json",
  });
  await save(page);
  const after = await savedBody(page.request, ws, doc.id);
  expect(after.content?.[0]?.attrs).toMatchObject({ id: "cancel-math", latex: "authorized final" });
  expect(after.content?.[1]).toEqual(before.content?.[1]);
});

test("Chromium IME engine keeps Korean source composition private until deliberate Apply on the same live document", async ({
  page,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "실제 Chromium 한글 조합", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "source-ime-body" },
          content: [{ type: "text", text: "원본 문단" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.focus();
  await page.keyboard.press("Control+a");
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send("Input.imeSetComposition", { text: "ㅎ", selectionStart: 1, selectionEnd: 1 });
    await expect(field).toHaveValue("ㅎ");
    await expect(page.locator('[data-editor-mode="rich"]')).toBeDisabled();
    await expect(page.getByRole("button", { name: "적용", exact: true })).toBeDisabled();
    await expectIdentity(page, 0);
    expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
    await cdp.send("Input.insertText", { text: "한글 연구 🧑‍💻" });
    await expect(field).toHaveValue("한글 연구 🧑‍💻");
    await expect(page.locator('[data-editor-mode="rich"]')).toBeEnabled();
    await expectIdentity(page, 0);
    await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await expectIdentity(page, 0);
    await expect(field).toHaveValue("원본 문단");
    await field.focus();
    await page.keyboard.press("Control+a");
    await field.evaluate((element) => {
      const events: unknown[] = [];
      (window as Window & { w3SourceIMEEvents?: unknown[] }).w3SourceIMEEvents = events;
      for (const name of [
        "compositionstart",
        "compositionupdate",
        "compositionend",
        "input",
        "blur",
      ])
        element.addEventListener(name, (event) =>
          events.push({
            type: event.type,
            value: (element as HTMLTextAreaElement).value,
            composing: event instanceof InputEvent ? event.isComposing : undefined,
          }),
        );
    });
    await cdp.send("Input.imeSetComposition", { text: "한", selectionStart: 1, selectionEnd: 1 });
    await cdp.send("Input.insertText", { text: "한글 조사" });
    await expect(field).toHaveValue("한글 조사");
    await expect(page.getByRole("button", { name: "적용", exact: true })).toBeEnabled();
    await page.getByRole("button", { name: "적용", exact: true }).click();
    await testInfo.attach("w3-source-native-IME-after-apply.json", {
      body: JSON.stringify(
        await page.evaluate(() => ({
          events: (window as Window & { w3SourceIMEEvents?: unknown[] }).w3SourceIMEEvents,
          panel: document.querySelector(".fvoci-source-panel")?.textContent,
          field: (
            document.querySelector(
              'textarea[aria-label="Markdown 직접 편집"]',
            ) as HTMLTextAreaElement
          ).value,
          viewComposing: (document.querySelector(".fvoci-editor .ProseMirror") as EditorElement)
            .editor.view.composing,
          document: (
            document.querySelector(".fvoci-editor .ProseMirror") as EditorElement
          ).editor.getJSON() as unknown,
        })),
      ),
      contentType: "application/json",
    });
    await selectMode(page, "rich");
    await expectBlocks(page, ["한글 조사"]);
    await expectIdentity(page);
    await save(page);
    const after = await savedBody(page.request, ws, doc.id);
    expect(after.content?.[0]?.attrs?.id).toBe(before.content?.[0]?.attrs?.id);
    expect(after.content?.[0]?.content).toEqual([{ type: "text", text: "한글 조사" }]);
    await caretAtEndOf(page, 0);
    await cdp.send("Input.imeSetComposition", { text: "ㅎ", selectionStart: 1, selectionEnd: 1 });
    await expect(editorOf(page)).toBeFocused();
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeDisabled();
    await page.locator('[data-editor-mode="markdown"]').evaluate((button) => {
      (button as HTMLButtonElement).click();
    });
    await expect(page.locator(".fvoci-editor")).toHaveAttribute("data-editor-mode-active", "rich");
    await expect(editorOf(page)).toBeFocused();
    await cdp.send("Input.insertText", { text: " 한글 동료" });
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeEnabled();
    await expectBlocks(page, ["한글 조사 한글 동료"]);
    await selectMode(page, "markdown");
    await expect(field).toHaveValue("한글 조사 한글 동료");
    await selectMode(page, "rich");
    await save(page);
    expect((await savedBody(page.request, ws, doc.id)).content?.[0]?.attrs?.id).toBe(
      before.content?.[0]?.attrs?.id,
    );
  } finally {
    await cdp.detach();
  }
});

test("Chromium rich IME engine blocks mode switching without focus loss and preserves Korean commit identity", async ({
  page,
}) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "실제 글쓰기 한글 조합", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "rich-ime-body" },
          content: [{ type: "text", text: "원본 문단" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await caretAtEndOf(page, 0);
  await editorOf(page).evaluate((root) => {
    root.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Process", keyCode: 229, bubbles: true }),
    );
  });
  await expect(page.locator('[data-editor-mode="markdown"]')).toBeDisabled();
  await expectIdentity(page, 0);
  await editorOf(page).evaluate((root) => {
    root.dispatchEvent(new KeyboardEvent("keyup", { key: "Process", keyCode: 229, bubbles: true }));
  });
  await expect(page.locator('[data-editor-mode="markdown"]')).toBeEnabled();
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send("Input.imeSetComposition", { text: "ㅎ", selectionStart: 1, selectionEnd: 1 });
    await expect(editorOf(page)).toBeFocused();
    expect(
      await editorOf(page).evaluate((root) => (root as EditorElement).editor.view.composing),
    ).toBe(true);
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeDisabled();
    await page.locator('[data-editor-mode="markdown"]').evaluate((button) => {
      (button as HTMLButtonElement).click();
    });
    await expect(page.locator(".fvoci-editor")).toHaveAttribute("data-editor-mode-active", "rich");
    await expect(editorOf(page)).toBeFocused();
    await cdp.send("Input.insertText", { text: " 한글 동료" });
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeEnabled();
    await expectBlocks(page, ["원본 문단 한글 동료"]);
    await expectIdentity(page);
    await save(page);
    const after = await savedBody(page.request, ws, doc.id);
    expect(after.content?.[0]?.attrs?.id).toBe(before.content?.[0]?.attrs?.id);
    expect(after.content?.[0]?.content).toEqual([{ type: "text", text: "원본 문단 한글 동료" }]);
    const math = await createDoc(page.request, ws, "NodeView 네이티브 한글 조합", {
      json: {
        type: "doc",
        content: [
          { type: "math", attrs: { id: "ime-native-math", latex: "x + y" } },
          {
            type: "paragraph",
            attrs: { id: "ime-native-tail" },
            content: [{ type: "text", text: "그대로" }],
          },
        ],
      },
    });
    await openDoc(page, math.path);
    await save(page);
    const mathBefore = await savedBody(page.request, ws, math.id);
    await recordIdentity(page);
    await page.getByTitle("수식 편집", { exact: true }).click();
    const field = page.getByRole("textbox", { name: "수식 LaTeX" });
    await field.focus();
    await page.keyboard.press("Control+a");
    await cdp.send("Input.imeSetComposition", { text: "ㅎ", selectionStart: 1, selectionEnd: 1 });
    await expect(field).toHaveValue("ㅎ");
    await expect(field).toBeFocused();
    // Installed NodeView.stopEvent excludes the textarea from PM, so the
    // shell must guard this field even though PM itself is not composing.
    expect(
      await editorOf(page).evaluate((root) => (root as EditorElement).editor.view.composing),
    ).toBe(false);
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeDisabled();
    await expectIdentity(page, 0);
    await cdp.send("Input.insertText", { text: "한글 비공개 수식" });
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeEnabled();
    await expectIdentity(page, 0);
    await editorOf(page).getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await expectIdentity(page, 0);
    expect(await savedBody(page.request, ws, math.id)).toEqual(mathBefore);
  } finally {
    await cdp.detach();
  }
});

test("actual identityless source range refuses Apply with precise warning and Cancel/viewing allocate no ID or content update", async ({
  page,
}) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "ID 없는 범위 보존", { markdown: "원본 문단" });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  expect(before.content?.[0]?.attrs?.id).toBeUndefined();
  await recordIdentity(page);
  for (const mode of ["markdown", "preview", "block", "rich"] as const)
    await selectMode(page, mode);
  await expectIdentity(page, 0);
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.fill("한글 조사");
  await page.getByRole("button", { name: "적용", exact: true }).click();
  await expect(page.locator(".fvoci-mode-warning")).toContainText(
    "content.0 · ID 없음 · id: Missing block identity",
  );
  await expectIdentity(page, 0);
  expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
  await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
  await expect(field).toHaveValue("원본 문단");
  await expectIdentity(page, 0);
  await selectMode(page, "rich");
  await expectBlocks(page, ["원본 문단"]);
  expect(
    await editorOf(page).evaluate(
      (root) => (root as EditorElement).editor.state.doc.child(0).attrs.id as unknown,
    ),
  ).toBe(null);
  await save(page);
  expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
});

test("ordinary rich and peer edits refresh clean Markdown entry while a dirty private draft remains stale until explicit Cancel", async ({
  page,
  browser,
  baseURL,
}) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "최신 생성 Markdown과 비공개 초안", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "fresh-generated-body" },
          content: [{ type: "text", text: "원본" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  await recordIdentity(page);
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await selectMode(page, "markdown");
  await expect(field).toHaveValue("원본");
  await selectMode(page, "rich");
  await caretAtEndOf(page, 0);
  await page.keyboard.type(" 로컬");
  await selectMode(page, "markdown");
  await expect(field).toHaveValue("원본 로컬");
  const peer = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(peer.page, doc.path);
    await caretAtEndOf(peer.page, 0);
    await peer.page.keyboard.type(" 동료");
    await expectBlocks(page, ["원본 로컬 동료"]);
    const observed = await editorOf(page).evaluate(
      (root) => (root as EditorElement).w3Witness?.updates,
    );
    if (observed === undefined) throw new Error("Missing live update witness");
    await selectMode(page, "rich");
    await selectMode(page, "markdown");
    await expect(field).toHaveValue("원본 로컬 동료");
    await expectIdentity(page, observed);
    await field.fill("작성 중인 비공개 초안 🧑‍💻");
    await selectMode(page, "rich");
    await caretAtEndOf(page, 0);
    await page.keyboard.type(" 추가");
    await expectBlocks(peer.page, ["원본 로컬 동료 추가"]);
    const dirtyObserved = await editorOf(page).evaluate(
      (root) => (root as EditorElement).w3Witness?.updates,
    );
    if (dirtyObserved === undefined) throw new Error("Missing dirty live update witness");
    await selectMode(page, "markdown");
    await expect(field).toHaveValue("작성 중인 비공개 초안 🧑‍💻");
    await expect(page.getByRole("button", { name: "적용", exact: true })).toBeDisabled();
    await expect(page.locator(".fvoci-source-panel [role=status]")).toBeVisible();
    await expectIdentity(page, dirtyObserved);
    await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await expect(field).toHaveValue("원본 로컬 동료 추가");
    await expectIdentity(page, dirtyObserved);
    await selectMode(page, "rich");
    await save(page);
    const after = await savedBody(page.request, ws, doc.id);
    expect(after.content?.[0]?.attrs?.id).toBe("fresh-generated-body");
    expect(after.content?.[0]?.content).toEqual([{ type: "text", text: "원본 로컬 동료 추가" }]);
  } finally {
    await peer.context.close();
  }
});

test("actual node and table cell bookmarks survive no-op modes, localized table source edit and later peer-cell undo without replacing cells", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "표 선택 동료 실행 취소", {
    json: {
      type: "doc",
      content: [
        { type: "math", attrs: { id: "selection-math", latex: "x + y" } },
        {
          type: "table",
          attrs: { id: "selection-table" },
          content: [
            {
              type: "tableRow",
              content: [
                {
                  type: "tableHeader",
                  attrs: { colspan: 1, rowspan: 1, colwidth: [160], background: "#abcdef" },
                  content: [
                    {
                      type: "paragraph",
                      attrs: { id: "cell-p-a" },
                      content: [{ type: "text", text: "셀 하나" }],
                    },
                  ],
                },
                {
                  type: "tableCell",
                  attrs: { colspan: 1, rowspan: 1, colwidth: [200] },
                  content: [
                    {
                      type: "paragraph",
                      attrs: { id: "cell-p-b" },
                      content: [{ type: "text", text: "셀 둘째" }],
                    },
                  ],
                },
              ],
            },
          ],
        },
        {
          type: "paragraph",
          attrs: { id: "selection-tail" },
          content: [{ type: "text", text: "그대로 문단" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  for (const kind of ["node", "cell"] as const) {
    const selection = await editorOf(page).evaluate((root, kind) => {
      const editor = (root as EditorElement).editor;
      if (kind === "node") {
        if (!editor.commands.setNodeSelection(0)) throw new Error("Node selection failed");
      } else {
        const cells: number[] = [];
        editor.state.doc.descendants((node, pos) => {
          if (node.type.name === "tableHeader" || node.type.name === "tableCell") cells.push(pos);
        });
        const [anchorCell, headCell] = cells;
        if (
          cells.length !== 2 ||
          anchorCell === undefined ||
          headCell === undefined ||
          !editor.commands.setCellSelection({ anchorCell, headCell })
        )
          throw new Error("Cell selection failed");
      }
      return editor.state.selection.toJSON() as unknown;
    }, kind);
    expect(selection).toHaveProperty("type", kind);
    for (const mode of ["markdown", "preview", "block", "rich"] as const)
      await selectMode(page, mode);
    expect(
      await editorOf(page).evaluate(
        (root) => (root as EditorElement).editor.state.selection.toJSON() as unknown,
      ),
    ).toEqual(selection);
    await expectIdentity(page, 0);
  }
  type CellElement = EditorElement & { w3Cells?: Y.XmlElement[] };
  await editorOf(page).evaluate((root) => {
    const element = root as CellElement;
    const table = element.w3Witness?.fragment.get(1) as Y.XmlElement;
    const row = table.get(0) as Y.XmlElement;
    element.w3Cells = [row.get(0) as Y.XmlElement, row.get(1) as Y.XmlElement];
  });
  const peer = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(peer.page, doc.path);
    await selectMode(page, "markdown");
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await field.fill((await field.inputValue()).replace("셀 하나", "셀 수정"));
    await page.getByRole("button", { name: "적용", exact: true }).click();
    await selectMode(page, "rich");
    await expect(editorOf(page).locator("th")).toContainText("셀 수정");
    expect(
      await editorOf(page).evaluate(
        (root) => (root as EditorElement).editor.state.selection.toJSON() as unknown,
      ),
    ).toHaveProperty("type", "cell");
    await expect(editorOf(peer.page).locator("th")).toContainText("셀 수정");
    await editorOf(peer.page).locator("td").click();
    await peer.page.keyboard.press("End");
    await peer.page.keyboard.type(" 동료");
    await expect(editorOf(page).locator("td")).toContainText("셀 둘째 동료");
    await editorOf(page).locator("th").click();
    await page.keyboard.press("Control+z");
    await expect(editorOf(page).locator("th")).toContainText("셀 하나");
    await expect(editorOf(page).locator("td")).toContainText("셀 둘째 동료");
    await expect(editorOf(peer.page).locator("th")).toContainText("셀 하나");
    await expect(editorOf(peer.page).locator("td")).toContainText("셀 둘째 동료");
    expect(
      await editorOf(page).evaluate((root) => {
        const element = root as CellElement;
        const table = element.w3Witness?.fragment.get(1) as Y.XmlElement;
        const row = table.get(0) as Y.XmlElement;
        return element.w3Cells?.[0] === row.get(0) && element.w3Cells[1] === row.get(1);
      }),
    ).toBe(true);
    await expectIdentity(page);
    await page.screenshot({ path: testInfo.outputPath("w3-table-peer-undo.png"), fullPage: true });
    await save(page);
    const after = await savedBody(page.request, ws, doc.id);
    expect(after.content?.[0]).toEqual(before.content?.[0]);
    expect(after.content?.[2]).toEqual(before.content?.[2]);
    expect(after.content?.[1]?.attrs?.id).toBe("selection-table");
    const row = after.content?.[1]?.content?.[0];
    expect(row?.content?.[0]?.attrs).toEqual(
      before.content?.[1]?.content?.[0]?.content?.[0]?.attrs,
    );
    expect(row?.content?.[1]?.attrs).toEqual(
      before.content?.[1]?.content?.[0]?.content?.[1]?.attrs,
    );
    expect(row?.content?.[0]?.content?.[0]).toEqual(
      before.content?.[1]?.content?.[0]?.content?.[0]?.content?.[0],
    );
    expect(row?.content?.[1]?.content?.[0]?.attrs?.id).toBe("cell-p-b");
    expect(row?.content?.[1]?.content?.[0]?.content).toEqual([
      { type: "text", text: "셀 둘째 동료" },
    ]);
  } finally {
    await peer.context.close();
  }
});

test("actual socket disconnect during native Math IME keeps the same connected field and blocks modes until composition ends", async ({
  page,
}, testInfo) => {
  let offline = false;
  let socket: WebSocketRoute | undefined;
  await page.routeWebSocket(/\/collab(?:\?|$)/, async (route) => {
    if (offline) {
      await route.close({ code: 1013, reason: "owned network pause" });
      return;
    }
    route.connectToServer();
    socket = route;
  });
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "接続中断と native 조합", {
    json: {
      type: "doc",
      content: [
        { type: "math", attrs: { id: "network-ime-math", latex: "x + y" } },
        {
          type: "paragraph",
          attrs: { id: "network-ime-tail" },
          content: [{ type: "text", text: "그대로" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await page.getByTitle("수식 편집", { exact: true }).click();
  const field = page.getByRole("textbox", { name: "수식 LaTeX" });
  await field.evaluate((element) => {
    (window as Window & { w3NetworkIMEField?: Element }).w3NetworkIMEField = element;
  });
  await field.focus();
  await page.keyboard.press("Control+a");
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send("Input.imeSetComposition", { text: "ㅎ", selectionStart: 1, selectionEnd: 1 });
    await expect(field).toHaveValue("ㅎ");
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeDisabled();
    if (!socket) throw new Error("Missing actual routed socket");
    offline = true;
    await socket.close({ code: 1012, reason: "owned temporary disconnect" });
    await expect(page.locator('[data-collab-status="disconnected"]')).toBeVisible();
    await expect(field).toBeFocused();
    expect(
      await field.evaluate(
        (element) =>
          (window as Window & { w3NetworkIMEField?: Element }).w3NetworkIMEField === element &&
          element.isConnected,
      ),
    ).toBe(true);
    await expectIdentity(page, 0);
    expect(
      await editorOf(page).evaluate((root) => (root as EditorElement).editor.view.composing),
    ).toBe(false);
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeDisabled();
    await expect(page.locator(".fvoci-editor")).toHaveAttribute("data-editor-mode-active", "rich");
    await page.screenshot({
      path: testInfo.outputPath("w3-network-native-composition.png"),
      fullPage: true,
    });
    await cdp.send("Input.insertText", { text: "한글 비공개 조합" });
    await expect(field).toHaveValue("한글 비공개 조합");
    await expect(page.locator('[data-editor-mode="markdown"]')).toBeEnabled();
    await editorOf(page).getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await expectIdentity(page, 0);
    offline = false;
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible({ timeout: 15000 });
    expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
    await expectIdentity(page, 0);
  } finally {
    offline = false;
    await cdp.detach();
  }
});

test("fresh real server readonly-authenticated join recovers write authority after actual metadata grant and matched save on the same Y.Doc", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "실제 readonly 인증에서 쓰기 복구", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "real-readonly-grant-body" },
          content: [{ type: "text", text: "권한 복구 전" }],
        },
      ],
    },
  });
  expect(
    (
      await page.request.patch(`/api/v1/workspaces/${ws}/documents/${doc.id}`, {
        data: { status: "archived" },
      })
    ).status(),
  ).toBe(200);
  const before = await savedBody(page.request, ws, doc.id);
  const reader = await newSignedInPage(browser, baseURL, admin);
  try {
    await openDoc(reader.page, doc.path);
    await recordIdentity(reader.page);
    await expect(editorOf(reader.page)).toHaveAttribute("contenteditable", "false");
    expect(
      await editorOf(reader.page).evaluate((root) => {
        const editor = (root as EditorElement).editor;
        const options = editor.extensionManager.extensions.find(
          (item) => item.name === "collaborationCaret",
        )?.options as Record<string, unknown> | undefined;
        const provider = options?.provider as HocuspocusProvider;
        return { authenticated: provider.isAuthenticated, scope: provider.authorizedScope };
      }),
    ).toEqual({ authenticated: true, scope: "readonly" });
    await editorOf(reader.page).evaluate((root) => {
      (window as Window & { w3RealGrant?: Witness }).w3RealGrant = (
        root as EditorElement
      ).w3Witness;
    });
    const originalClientId = await editorOf(reader.page).evaluate(
      (root) => (root as EditorElement).w3Witness?.doc.clientID,
    );
    await testInfo.attach("w3-real-readonly-join.json", {
      body: JSON.stringify(await readLiveRuntime(reader.page)),
      contentType: "application/json",
    });
    for (const mode of ["markdown", "preview", "block", "rich"] as const)
      await selectMode(reader.page, mode);
    await expectIdentity(reader.page, 0);
    expect(
      (
        await page.request.patch(`/api/v1/workspaces/${ws}/documents/${doc.id}`, {
          data: { status: "draft" },
        })
      ).status(),
    ).toBe(200);
    await refetchActualQuery(reader.page, ["document"]);
    try {
      await expect(editorOf(reader.page)).toHaveAttribute("contenteditable", "true");
    } finally {
      await testInfo.attach("w3-real-readonly-grant.json", {
        body: JSON.stringify(await readLiveRuntime(reader.page)),
        contentType: "application/json",
      });
    }
    expect(
      await editorOf(reader.page).evaluate((root) => {
        const editor = (root as EditorElement).editor;
        const options = editor.extensionManager.extensions.find(
          (item) => item.name === "collaboration",
        )?.options as Record<string, unknown> | undefined;
        const ydoc = options?.document as Y.Doc;
        return ydoc === (window as Window & { w3RealGrant?: Witness }).w3RealGrant?.doc;
      }),
    ).toBe(true);
    await expectIdentity(reader.page, 0);
    expect(
      await editorOf(reader.page).evaluate((root) => {
        const witness = (root as EditorElement).w3Witness;
        const provider = witness?.provider as HocuspocusProvider;
        return {
          clientId: witness?.doc.clientID,
          authenticated: provider.isAuthenticated,
          scope: provider.authorizedScope,
        };
      }),
    ).toEqual({ clientId: originalClientId, authenticated: true, scope: "read-write" });
    await selectMode(reader.page, "markdown");
    await reader.page
      .getByRole("textbox", { name: "Markdown 직접 편집" })
      .fill("실제 쓰기 권한으로 한글 수정");
    await reader.page.getByRole("button", { name: "적용", exact: true }).click();
    await selectMode(reader.page, "rich");
    await expectBlocks(reader.page, ["실제 쓰기 권한으로 한글 수정"]);
    await save(reader.page);
    const after = await savedBody(reader.page.request, ws, doc.id);
    expect(after.content?.[0]?.attrs?.id).toBe(before.content?.[0]?.attrs?.id);
    expect(after.content?.[0]?.content).toEqual([
      { type: "text", text: "실제 쓰기 권한으로 한글 수정" },
    ]);
    const sql = restrictedDbBody(ws, doc.id);
    expect(sql).toHaveProperty("content", after);
    await testInfo.attach("w3-real-readonly-grant-app-role-DB.json", {
      body: JSON.stringify(sql),
      contentType: "application/json",
    });
    const newest = await newSignedInPage(browser, baseURL, admin);
    try {
      await openDoc(newest.page, doc.path);
      expect(await savedBody(newest.page.request, ws, doc.id)).toEqual(after);
      await expectBlocks(newest.page, ["실제 쓰기 권한으로 한글 수정"]);
    } finally {
      await newest.context.close();
    }
  } finally {
    await reader.context.close();
  }
});

for (const kind of ["project", "task"] as const) {
  test(`${kind} host copies actual newest marked body only after its own genuine readonly HTTP response and preserves live authority`, async ({
    page,
    browser,
    baseURL,
  }, testInfo) => {
    await authenticatedHome(page);
    const ws = await workspaceId(page.request);
    const key = kind === "project" ? "WC3" : "TC3";
    const projectResponse = await page.request.post(`/api/v1/workspaces/${ws}/projects`, {
      data: { key, name: `${kind} 최신 본문 검토`, visibility: "private" },
    });
    expect(projectResponse.status()).toBe(201);
    const project = await readJson(projectResponse, flowSchemas.project);
    const created = await page.request.post(
      `/api/v1/workspaces/${ws}/projects/${project.id}/${kind === "project" ? "documents" : "tasks"}`,
      {
        data:
          kind === "project"
            ? { parentId: project.rootDocumentId, title: "프로젝트 본문" }
            : { title: "태스크 본문" },
      },
    );
    expect(created.status()).toBe(201);
    const resource = await readJson(
      created,
      kind === "project" ? flowSchemas.createdDocument : flowSchemas.numbered,
    );
    const path =
      kind === "project"
        ? (resource as { displayId?: unknown }).displayId
        : `${key}-${String(resource.number)}`;
    if (typeof path !== "string") throw new Error("Missing actual resource display path");
    const metadataUrl =
      kind === "project"
        ? `/api/v1/workspaces/${ws}/projects/${project.id}/documents/${resource.id}`
        : `/api/v1/workspaces/${ws}/tasks/${resource.id}`;
    const bodyUrl = kind === "project" ? `${metadataUrl}/body` : metadataUrl;
    await openDoc(page, `/w/${admin.workspaceSlug}/${path}`);
    await editorOf(page).click();
    await page.keyboard.press("Control+b");
    await page.keyboard.type(`${kind} 최신 한글 🧑‍💻`);
    const host = kind === "task" ? page.getByTestId("task-body") : page.locator("article").first();
    await host.getByRole("button", { name: "저장", exact: true }).click();
    await expect(host.locator('[data-collab-persisted="true"]')).toBeVisible({ timeout: 15000 });
    const beforeResponse = await page.request.get(bodyUrl);
    expect(beforeResponse.status()).toBe(200);
    const before = await readJson(beforeResponse, flowSchemas.body);
    expect(
      (
        await page.request.patch(metadataUrl, {
          data: kind === "project" ? { status: "archived" } : { archived: true },
        })
      ).status(),
    ).toBe(200);
    const reader = await newSignedInPage(browser, baseURL, admin, {
      permissions: ["clipboard-read", "clipboard-write"],
    });
    let received = false;
    let release: () => void = () => {};
    const held = new Promise<void>((done) => {
      release = done;
    });
    let actualBody: unknown;
    try {
      await openDoc(reader.page, `/w/${admin.workspaceSlug}/${path}`);
      await recordIdentity(reader.page);
      await expect(editorOf(reader.page)).toHaveAttribute("contenteditable", "false");
      await reader.page.route(`**${bodyUrl}`, async (route) => {
        const response = await route.fetch();
        expect(response.status()).toBe(200);
        actualBody = (await response.json()) as unknown;
        received = true;
        await held;
        await route.fulfill({ response });
      });
      await selectMode(reader.page, "markdown");
      await reader.page.evaluate(() => navigator.clipboard.writeText("host readonly sentinel"));
      await reader.page.getByRole("button", { name: "저장된 현재 문서 복사" }).click();
      await expect.poll(() => received).toBe(true);
      expect(actualBody).toMatchObject({ contentJson: before.contentJson });
      expect(await reader.page.evaluate(() => navigator.clipboard.readText())).toBe(
        "host readonly sentinel",
      );
      release();
      try {
        // Full-document export contract: md.ts appends exactly one terminal LF.
        await expect
          .poll(() => reader.page.evaluate(() => navigator.clipboard.readText()))
          .toBe(`**${kind} 최신 한글 🧑‍💻**\n`);
      } finally {
        await testInfo.attach(`w3-${kind}-readonly-live.json`, {
          body: JSON.stringify({
            before: before.contentJson,
            actualBody,
            live: await readLiveRuntime(reader.page),
          }),
          contentType: "application/json",
        });
      }
      await selectMode(reader.page, "preview");
      await expect(reader.page.locator(".fvoci-mode-preview")).toContainText(
        `${kind} 최신 한글 🧑‍💻`,
      );
      await selectMode(reader.page, "rich");
      await expectIdentity(reader.page, 0);
      const afterResponse = await reader.page.request.get(bodyUrl);
      expect(afterResponse.status()).toBe(200);
      expect((await readJson(afterResponse, flowSchemas.body)).contentJson).toEqual(
        before.contentJson,
      );
    } finally {
      release();
      await reader.page.unroute(`**${bodyUrl}`);
      await reader.context.close();
    }
  });
}

test("real router navigation protects a private Markdown draft and actual logout retires it without saving", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "초안 이동과 로그아웃", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "leave-source-body" },
          content: [{ type: "text", text: "보존할 본문" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  const before = await savedBody(page.request, ws, doc.id);
  await recordIdentity(page);
  await page.evaluate(() => {
    const root = document.querySelector(".tiptap") as EditorElement;
    (window as Window & { w3Leaving?: Witness }).w3Leaving = root.w3Witness;
  });
  await selectMode(page, "markdown");
  const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
  await field.fill("저장되지 않은 한글 비공개 🧑‍💻");
  await page.getByRole("link", { name: "홈", exact: true }).first().click();
  const dialog = page.getByRole("dialog", { name: "Markdown 초안을 두고 이동할까요?" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "계속 편집", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`${doc.path}$`));
  await expect(field).toHaveValue("저장되지 않은 한글 비공개 🧑‍💻");
  await expectIdentity(page, 0);
  await page.screenshot({
    path: testInfo.outputPath("w3-private-draft-navigation.png"),
    fullPage: true,
  });
  await page.getByRole("link", { name: "홈", exact: true }).first().click();
  await dialog.getByRole("button", { name: "초안 버리고 이동", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}$`));
  await expect
    .poll(() =>
      page.evaluate(() => {
        const old = (window as Window & { w3Leaving?: Witness }).w3Leaving;
        if (!old) throw new Error("Missing retired room witness");
        const provider = old.provider as HocuspocusProvider;
        return {
          destroyed: old.editor.isDestroyed,
          attached: provider.isAttached,
          reconnecting: provider.configuration.websocketProvider.shouldConnect,
          updates: old.updates,
        };
      }),
    )
    .toEqual({ destroyed: true, attached: false, reconnecting: false, updates: 0 });
  expect(await savedBody(page.request, ws, doc.id)).toEqual(before);
  await page.goBack();
  await expect(editorOf(page)).toBeVisible();
  await expectBlocks(page, ["보존할 본문"]);
  expect(
    await editorOf(page).evaluate((root) => {
      const old = (window as Window & { w3Leaving?: Witness }).w3Leaving;
      const options = (root as EditorElement).editor.extensionManager.extensions.find(
        (item) => item.name === "collaboration",
      )?.options as Record<string, unknown> | undefined;
      return old?.doc !== options?.document;
    }),
  ).toBe(true);
  await selectMode(page, "markdown");
  await expect(field).toHaveValue("보존할 본문");
  await field.fill("로그아웃으로 폐기할 비공개 초안");
  const observer = await newSignedInPage(browser, baseURL, admin);
  try {
    await page.getByRole("button", { name: "로그아웃", exact: true }).click();
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByLabel("이메일")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Markdown 직접 편집" })).toHaveCount(0);
    expect(await savedBody(observer.page.request, ws, doc.id)).toEqual(before);
    await openDoc(observer.page, doc.path);
    await expectBlocks(observer.page, ["보존할 본문"]);
    await selectMode(observer.page, "markdown");
    await expect(observer.page.getByRole("textbox", { name: "Markdown 직접 편집" })).toHaveValue(
      "보존할 본문",
    );
  } finally {
    await observer.context.close();
  }
});

test("pending rich save across mode entry cannot mark a newer Markdown prefix saved before its own matched ACK", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  const gate = await sourceAckGate(page);
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "모드 진입 중 저장", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "pending-mode-body" },
          content: [{ type: "text", text: "기준" }],
        },
      ],
    },
  });
  try {
    await openDoc(page, doc.path);
    await observeActualAckDelivery(page);
    await recordIdentity(page);
    await caretAtEndOf(page, 0);
    await page.keyboard.type(" 첫 수정");
    await page.getByRole("button", { name: "저장", exact: true }).click();
    await expect.poll(() => gate.held.length).toBe(1);
    await selectMode(page, "markdown");
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await expect(field).toHaveValue("기준 첫 수정");
    await expect(page.locator('[data-collab-persisted="false"]')).toBeVisible();
    await field.fill("기준 가장 최신 한글");
    await page.getByRole("button", { name: "적용", exact: true }).click();
    const old = gate.held[0];
    if (!old) throw new Error("Missing original rich-save ACK");
    gate.release(0);
    await expectAckDelivered(page, `persisted:${old.id}`);
    await expect(page.locator('[data-collab-persisted="false"]')).toBeVisible();
    await expectBlocks(page, ["기준 가장 최신 한글"]);
    await expectIdentity(page);
    await selectMode(page, "rich");
    await page.getByRole("button", { name: "저장", exact: true }).click();
    await expect.poll(() => gate.held.length).toBe(2);
    gate.release(1);
    await expect(page.locator('[data-collab-persisted="true"]')).toBeVisible();
    const after = await savedBody(page.request, ws, doc.id);
    expect(after.content?.[0]?.attrs?.id).toBe("pending-mode-body");
    expect(after.content?.[0]?.content).toEqual([{ type: "text", text: "기준 가장 최신 한글" }]);
    const sql = restrictedDbBody(ws, doc.id);
    expect(sql).toHaveProperty("content", after);
    await testInfo.attach("w3-pending-mode-new-prefix-app-role-DB.json", {
      body: JSON.stringify(sql),
      contentType: "application/json",
    });
    const fresh = await newSignedInPage(browser, baseURL, admin);
    try {
      await openDoc(fresh.page, doc.path);
      await expectBlocks(fresh.page, ["기준 가장 최신 한글"]);
      expect(await savedBody(fresh.page.request, ws, doc.id)).toEqual(after);
    } finally {
      await fresh.context.close();
    }
  } finally {
    gate.releaseAll();
  }
});

async function nativeFileDrop(page: Page, path: string): Promise<void> {
  const block = blockAt(page, 0);
  await block.scrollIntoViewIfNeeded();
  const point = await block.evaluate((element) => {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    let text = walker.nextNode();
    if (!text) throw new Error("Missing native upload text target");
    while (walker.nextNode()) text = walker.currentNode;
    const range = document.createRange();
    range.setStart(text, text.textContent?.length ?? 0);
    range.collapse(true);
    const rect = range.getBoundingClientRect();
    return { x: rect.x + 1, y: rect.y + rect.height / 2 };
  });
  const session = await page.context().newCDPSession(page);
  try {
    const data = { items: [], files: [path], dragOperationsMask: 1 };
    for (const type of ["dragEnter", "dragOver", "drop"] as const)
      await session.send("Input.dispatchDragEvent", { type, ...point, data });
  } finally {
    await session.detach();
  }
}

test("real deferred native upload invalidates a private Markdown proposal and its next pending upload aborts on actual unmount", async ({
  page,
}, testInfo) => {
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "첨부 완료와 초안 수명", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "upload-source-p" },
          content: [{ type: "text", text: "첨부 기준" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  await save(page);
  await recordIdentity(page);
  const path = testInfo.outputPath("첨부 한글.txt");
  writeFileSync(path, "실제 첨부 내용 🧑‍💻\n");
  let release: () => void = () => {};
  let received = 0;
  let routeFinished = 0;
  let held = new Promise<void>((done) => {
    release = done;
  });
  const uploadUrl = `**/documents/${doc.id}/uploads`;
  const routeErrors: string[] = [];
  await page.route(uploadUrl, async (route) => {
    received++;
    await held;
    await route.continue().catch((error: unknown) => {
      routeErrors.push(error instanceof Error ? error.message : String(error));
    });
    routeFinished++;
  });
  try {
    await nativeFileDrop(page, path);
    await expect.poll(() => received).toBe(1);
    await expect(page.getByRole("progressbar")).toHaveCount(1);
    await selectMode(page, "markdown");
    const field = page.getByRole("textbox", { name: "Markdown 직접 편집" });
    await field.fill("업로드 전에 만든 비공개 한글 초안");
    await expectIdentity(page, 0);
    release();
    await expect(editorOf(page).locator('[data-state="stored"]')).toHaveCount(1);
    await expect(page.getByRole("button", { name: "적용", exact: true })).toBeDisabled();
    await expect(field).toHaveValue("업로드 전에 만든 비공개 한글 초안");
    await expect(page.locator(".fvoci-source-panel [role=status]")).toHaveText(
      "문서가 변경되었습니다. 초안을 취소하고 최신 내용을 다시 열어 주세요.",
    );
    await page.getByRole("button", { name: "취소 · 최신 내용 열기" }).click();
    await selectMode(page, "rich");
    await expect(editorOf(page).locator(".afn-attachment-name")).toHaveText("첨부 한글.txt");
    await expectIdentity(page);
    await save(page);
    const after = await savedBody(page.request, ws, doc.id);
    expect(after.content?.[0]?.attrs?.id).toBe("upload-source-p");
    expect(after.content?.[0]?.content).toEqual([{ type: "text", text: "첨부 기준" }]);
    const attachments = after.content?.filter((node) => node.type === "attachment");
    expect(attachments).toHaveLength(1);
    const attachment = attachments?.[0];
    if (typeof attachment?.attrs?.id !== "string") throw new Error("Missing actual attachment ID");
    await expect(editorOf(page).locator('a[data-state="stored"]')).toHaveAttribute(
      "href",
      `/api/v1/workspaces/${ws}/attachments/${attachment.attrs.id}/download`,
    );
    await selectMode(page, "markdown");
    await expect(field).toHaveValue(new RegExp(attachment.attrs.id));
    await selectMode(page, "rich");
    held = new Promise<void>((done) => {
      release = done;
    });
    const failures: string[] = [];
    page.on("requestfailed", (request) => {
      if (request.url().endsWith(`/documents/${doc.id}/uploads`))
        failures.push(request.failure()?.errorText ?? "missing reason");
    });
    await nativeFileDrop(page, path);
    await expect.poll(() => received).toBe(2);
    await expect(page.getByRole("progressbar")).toHaveCount(1);
    await page.getByRole("link", { name: "홈", exact: true }).first().click();
    await expect(page).toHaveURL(new RegExp(`/w/${admin.workspaceSlug}$`));
    await expect.poll(() => failures.length).toBe(1);
    expect(failures[0]).toContain("ERR_ABORTED");
    release();
    await expect.poll(() => routeFinished).toBe(2);
    expect(await savedBody(page.request, ws, doc.id)).toEqual(after);
    await page.goBack();
    await expect(editorOf(page)).toBeVisible();
    await expect(editorOf(page).locator('[data-state="stored"]')).toHaveCount(1);
    expect(await savedBody(page.request, ws, doc.id)).toEqual(after);
    await testInfo.attach("w3-deferred-upload-epoch-unmount.json", {
      body: JSON.stringify({ beforeUnmount: after, failures, routeErrors }),
      contentType: "application/json",
    });
  } finally {
    release();
    await page.unroute(uploadUrl);
  }
});

test("actual served editor JavaScript CSS and complete notices match this frozen production dist", async ({
  page,
}, testInfo) => {
  const witnesses: {
    path: string;
    status: number;
    bytes: number;
    sha256: string;
    expectedSha256: string;
  }[] = [];
  const pending: Promise<void>[] = [];
  const sha = (value: Uint8Array) => createHash("sha256").update(value).digest("hex");
  page.on("response", (response) => {
    const path = new URL(response.url()).pathname;
    if (!/^\/assets\/[^/]+\.(?:js|css)$/.test(path)) return;
    pending.push(
      response.body().then((body) => {
        const expected = readFileSync(new URL(`../dist${path}`, import.meta.url));
        witnesses.push({
          path,
          status: response.status(),
          bytes: body.length,
          sha256: sha(body),
          expectedSha256: sha(expected),
        });
      }),
    );
  });
  await authenticatedHome(page);
  const ws = await workspaceId(page.request);
  const doc = await createDoc(page.request, ws, "실제 제공된 편집기 자산", {
    json: {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "asset-witness-body" },
          content: [{ type: "text", text: "한글 자산 🧑‍💻" }],
        },
      ],
    },
  });
  await openDoc(page, doc.path);
  for (const mode of ["markdown", "preview", "block", "rich"] as const)
    await selectMode(page, mode);
  await Promise.all(pending);
  expect(witnesses.some((item) => item.path.endsWith(".js"))).toBe(true);
  expect(witnesses.some((item) => item.path.endsWith(".css"))).toBe(true);
  for (const item of witnesses) {
    expect(item.status).toBe(200);
    expect(item.sha256).toBe(item.expectedSha256);
  }
  const notices = await page.request.get("/open-source-licenses.txt");
  expect(notices.status()).toBe(200);
  const servedNotices = await notices.body();
  const expectedNotices = readFileSync(
    new URL("../dist/open-source-licenses.txt", import.meta.url),
  );
  expect(servedNotices).toEqual(expectedNotices);
  expect(servedNotices.toString()).toContain("## launder - 1.7.1 (MIT)");
  expect(servedNotices.toString()).toContain("## remark-math - 6.0.0 (MIT)");
  const source = spawnSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" });
  expect(source.status).toBe(0);
  await testInfo.attach("w3-actual-served-assets-and-notices.json", {
    body: JSON.stringify({
      sourceSHA: source.stdout.trim(),
      witnesses,
      notices: {
        bytes: servedNotices.length,
        sha256: sha(servedNotices),
        expectedSha256: sha(expectedNotices),
      },
    }),
    contentType: "application/json",
  });
});
