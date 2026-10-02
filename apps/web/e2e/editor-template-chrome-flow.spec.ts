// Official Nuxt UI editor chrome on FVOCI's existing Tiptap/Yjs host.
// Real browser, Rust server, DB, peer and persisted ACK; no demo document/store.
import { spawnSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";
import type { Editor } from "@tiptap/core";
import type { Transaction } from "@tiptap/pm/state";
import type { HocuspocusProvider } from "@hocuspocus/provider";
import type * as Y from "yjs";
import { readJson, flowSchemas, login, watchCspViolations } from "./helpers";
import {
  admin,
  member,
  blockAt,
  caretAtEndOf,
  createDoc,
  editorOf,
  newSignedInPage,
  expectBlocks,
  openDoc,
  save,
  savedBody,
  type TiptapNode,
  setupInstance,
  watchIconRequests,
  workspaceId,
} from "./workspace-wiki-vue-editor";

test.describe.configure({ mode: "serial" });
test.beforeAll(async ({ browser, baseURL }) => {
  await setupInstance(browser, baseURL);
});

type TemplateObserver = { checkpoint(stage: string): void; stop(): unknown };
type ObservedWindow = Window & { __w3TemplateObserver?: TemplateObserver };

async function observeTemplateSelection(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const frames: unknown[] = [];
    const critical: unknown[] = [];
    const ownerChanges: unknown[] = [];
    const observedMismatches: unknown[] = [];
    const actionBoundaries: unknown[] = [];
    const totals = {
      frames: 0,
      critical: 0,
      ownerChanges: 0,
      observedMismatches: 0,
      actionBoundaries: 0,
    };
    const dropped = {
      frames: 0,
      critical: 0,
      ownerChanges: 0,
      observedMismatches: 0,
      actionBoundaries: 0,
    };
    const append = (key: keyof typeof totals, values: unknown[], value: unknown, limit: number) => {
      totals[key]++;
      values.push(value);
      if (values.length > limit) {
        // Keep the first latch and bounded tail; frames are a rolling window.
        values.splice(key === "frames" ? 0 : 1, 1);
        dropped[key]++;
      }
    };
    let firstObservedState: unknown;
    let firstRetiredEvent: unknown;
    const ids = new WeakMap<object, number>();
    let nextId = 0;
    const token = (value?: object | null) => {
      if (!value) return null;
      if (!ids.has(value)) ids.set(value, ++nextId);
      return ids.get(value);
    };
    let bound: Editor | undefined;
    let ydoc: Y.Doc | undefined;
    let provider: HocuspocusProvider | undefined;
    let previousOwner: string | undefined;
    let bindingGeneration = 0;
    let generationUpdates = 0;
    let generationLocalUpdates = 0;
    let previousNativeId: unknown;
    let previousBubble = false;

    let updates = 0;
    let localUpdates = 0;
    let callbacks:
      | {
          transaction: (payload: { transaction: Transaction }) => void;
          update: (bytes: Uint8Array, origin: unknown, doc: Y.Doc, tr: Y.Transaction) => void;
          authenticated: () => void;
          status: () => void;
          synced: () => void;
        }
      | undefined;
    let stopped = false;
    let animation = 0;
    const detach = () => {
      if (!callbacks) return;
      bound?.off("transaction", callbacks.transaction);
      ydoc?.off("update", callbacks.update);
      provider?.off("authenticated", callbacks.authenticated);
      provider?.off("status", callbacks.status);
      provider?.off("synced", callbacks.synced);
      callbacks = undefined;
    };
    const record = (
      frame: { at: number; stage: string; [key: string]: unknown },
      retain: boolean,
    ) => {
      append("frames", frames, frame, 512);
      if (retain) append("critical", critical, frame, 256);
    };
    const capture = (
      stage: string,
      retain = false,
      observedUpdate?: { doc: Y.Doc; local: boolean },
      eventBindingGeneration?: number,
    ) => {
      if (stopped) return;
      try {
        const root = document.querySelector<HTMLElement & { editor?: Editor }>(
          ".fvoci-editor .ProseMirror",
        );
        const mounted = root?.editor;
        const current = mounted && !mounted.isDestroyed ? mounted : undefined;
        const documentOptions = current?.extensionManager.extensions.find(
          (extension) => extension.name === "collaboration",
        )?.options as { document?: Y.Doc } | undefined;
        const caretOptions = current?.extensionManager.extensions.find(
          (extension) => extension.name === "collaborationCaret",
        )?.options as { provider?: HocuspocusProvider } | undefined;
        const currentDoc = documentOptions?.document;
        const currentProvider = caretOptions?.provider;
        if (current !== bound || currentDoc !== ydoc || currentProvider !== provider) {
          detach();
          bound = current;
          ydoc = currentDoc;
          provider = currentProvider;
          bindingGeneration++;
          generationUpdates = 0;
          generationLocalUpdates = 0;
          const generation = bindingGeneration;
          callbacks = {
            transaction: ({ transaction: tr }) => {
              capture(
                `transaction:doc=${String(tr.docChanged)}:selection=${String(tr.selectionSet)}`,
                tr.docChanged,
                undefined,
                generation,
              );
            },
            update: (_bytes, _origin, doc, tr) => {
              capture(
                `Yupdate:local=${String(tr.local)}`,
                true,
                { doc, local: tr.local },
                generation,
              );
            },
            authenticated: () => {
              capture("provider:authenticated", true, undefined, generation);
            },
            status: () => {
              capture("provider:status", true, undefined, generation);
            },
            synced: () => {
              capture("provider:synced", true, undefined, generation);
            },
          };
          bound?.on("transaction", callbacks.transaction);
          ydoc?.on("update", callbacks.update);
          provider?.on("authenticated", callbacks.authenticated);
          provider?.on("status", callbacks.status);
          provider?.on("synced", callbacks.synced);
        }
        const at = performance.now();
        const fragment = ydoc?.share.get("prosemirror") as Y.XmlFragment | undefined;
        const node = fragment?.toArray()[0];
        const owner = JSON.stringify([
          token(root),
          token(mounted),
          token(current?.view.dom),
          token(ydoc),
          token(provider),
          token(fragment),
          token(node),
        ]);
        const ownerChanged = previousOwner !== undefined && owner !== previousOwner;
        if (ownerChanged) {
          append(
            "ownerChanges",
            ownerChanges,
            {
              stage,
              at,
              previousOwner,
              owner,
              bindingGeneration,
            },
            128,
          );
        }
        previousOwner = owner;
        const retiredEvent =
          eventBindingGeneration !== undefined && eventBindingGeneration !== bindingGeneration;
        if (!current || !root) {
          const frame = {
            at,
            stage,
            owner,
            bindingGeneration,
            eventBindingGeneration,
            retiredEvent,
            unavailable: mounted?.isDestroyed ? "destroyed-editor" : "missing-editor",
          };
          if (retiredEvent && firstRetiredEvent === undefined) firstRetiredEvent = frame;
          record(frame, retain || ownerChanged);
          return;
        }
        if (observedUpdate && observedUpdate.doc === ydoc && !retiredEvent) {
          updates++;
          generationUpdates++;
          if (observedUpdate.local) {
            localUpdates++;
            generationLocalUpdates++;
          }
        }
        const state = current.view.state;
        const native = window.getSelection();
        const inside = Boolean(
          native?.anchorNode &&
          native.focusNode &&
          root.contains(native.anchorNode) &&
          root.contains(native.focusNode),
        );
        let nativePositions: { anchor: number; head: number } | { unknown: string } | null = null;
        if (inside && native?.anchorNode && native.focusNode) {
          try {
            nativePositions = {
              anchor: current.view.posAtDOM(native.anchorNode, native.anchorOffset),
              head: current.view.posAtDOM(native.focusNode, native.focusOffset),
            };
          } catch (error) {
            nativePositions = { unknown: String(error) };
          }
        }
        const nativeId: unknown = node && "getAttribute" in node ? node.getAttribute("id") : null;
        const bubble = document.querySelector<HTMLElement>("[data-fvoci-bubble]");
        const bubbleChanged = Boolean(bubble) !== previousBubble;
        const idChanged = nativeId !== previousNativeId;
        previousBubble = Boolean(bubble);
        previousNativeId = nativeId;
        const frame = {
          at,
          stage,
          owner,
          bindingGeneration,
          generationUpdates,
          generationLocalUpdates,
          eventBindingGeneration,
          retiredEvent,
          retiredUpdate: Boolean(observedUpdate && observedUpdate.doc !== ydoc),
          clientID: ydoc?.clientID ?? null,
          native: {
            inside,
            text: native?.toString() ?? null,
            positions: nativePositions,
            anchorNode: token(native?.anchorNode),
            anchorOffset: native?.anchorOffset,
            focusNode: token(native?.focusNode),
            focusOffset: native?.focusOffset,
          },
          pm: {
            anchor: state.selection.anchor,
            head: state.selection.head,
            empty: state.selection.empty,
            type: (state.selection.toJSON() as { type?: unknown }).type,
            editorAnchor: current.state.selection.anchor,
            editorHead: current.state.selection.head,
            marks: state.storedMarks?.map((mark) => mark.toJSON() as unknown) ?? null,
          },
          focus: {
            activeTag: document.activeElement?.tagName,
            activeLabel: document.activeElement?.getAttribute("aria-label"),
            editor: current.view.hasFocus(),
            domEditable: root.contentEditable,
            editorEditable: current.isEditable,
            composing: current.view.composing,
          },
          auth: {
            authenticated: provider?.isAuthenticated ?? null,
            scope: provider?.authorizedScope ?? null,
            synced: provider?.synced ?? null,
            status: provider?.configuration.websocketProvider.status ?? null,
          },
          nativeId,
          updates,
          localUpdates,
          bubble: bubble
            ? { visibility: bubble.style.visibility, opacity: bubble.style.opacity }
            : null,
          dialog: Boolean(document.querySelector('[role="dialog"]')),
          ...(retain || bubbleChanged || idChanged
            ? { pmDocument: state.doc.toJSON() as unknown }
            : {}),
        };
        if (firstObservedState === undefined) firstObservedState = frame;
        if (retiredEvent && firstRetiredEvent === undefined) firstRetiredEvent = frame;
        record(frame, retain || bubbleChanged || idChanged || ownerChanged);
        if (
          nativePositions &&
          "anchor" in nativePositions &&
          (nativePositions.anchor !== state.selection.anchor ||
            nativePositions.head !== state.selection.head)
        ) {
          // Native-to-PM settling is observed, not classified as a defect here.
          append(
            "observedMismatches",
            observedMismatches,
            {
              at: frame.at,
              stage,
              owner,
              bindingGeneration,
              nativePositions,
              pm: frame.pm,
            },
            256,
          );
        }
      } catch (error) {
        append("critical", critical, { at: performance.now(), stage, unknown: String(error) }, 256);
      }
    };
    const events = ["keydown", "keyup", "selectionchange", "focusin", "focusout", "pointerdown"];
    const event = (value: Event) => {
      const legacyKeyCode: unknown =
        value instanceof KeyboardEvent ? Reflect.get(value, "keyCode") : null;
      const stage = `${value.type}:${value instanceof KeyboardEvent ? `${value.key}:shift=${String(value.shiftKey)}:composing=${String(value.isComposing)}:keyCode=${String(legacyKeyCode)}` : ""}`;
      capture(stage, true);
      queueMicrotask(() => {
        if (!stopped) capture(`${stage}:microtask`, true);
      });
    };
    for (const name of events) document.addEventListener(name, event, true);
    window.addEventListener("pagehide", event, true);
    const tick = () => {
      if (stopped) return;
      capture("frame");
      animation = requestAnimationFrame(tick);
    };
    animation = requestAnimationFrame(tick);
    (window as ObservedWindow).__w3TemplateObserver = {
      checkpoint: (stage) => {
        capture(stage, true);
        const latest = frames.at(-1);
        if (latest) append("actionBoundaries", actionBoundaries, latest, 16);
      },
      stop() {
        capture("finally", true);
        stopped = true;
        cancelAnimationFrame(animation);
        for (const name of events) document.removeEventListener(name, event, true);
        window.removeEventListener("pagehide", event, true);
        detach();
        return {
          frames,
          critical,
          ownerChanges,
          observedMismatches,
          actionBoundaries,
          firstObservedState,
          firstRetiredEvent,
          totals,
          dropped,
          bindingGeneration,
          earlyBindingNotCaptured: true,
          navigationContinuity: "current-document-only",
          updates,
          localUpdates,
        };
      },
    };
  });
}

test("non-editor Vue screens do not load the editor host or its collaboration plugins", async ({
  page,
}) => {
  const assets = new Set<string>();
  const captureAssets = async () => {
    const paths = await page.evaluate(() =>
      performance
        .getEntriesByType("resource")
        .map((entry) => new URL(entry.name).pathname)
        .filter((path) => /^\/assets\/[^/]+\.(js|css)$/.test(path)),
    );
    expect(paths.some((path) => path.endsWith(".js"))).toBe(true);
    for (const path of paths) assets.add(path);
  };
  await login(page, admin.email, admin.password);
  await expect(page.getByRole("button", { name: "로그아웃", exact: true })).toBeVisible();
  await captureAssets();
  // Workspace home is still React at this dispatched base; exercise the
  // actual Vue HTML-sink route rather than attributing React CSS to Vue.
  await page.goto("/legal/privacy");
  await expect(page.getByRole("heading").first()).toBeVisible();
  await expect(page.locator("#root")).toHaveClass(/isolate/);
  await captureAssets();
  const wsId = await workspaceId(page.request);
  const project = await page.request.post(`/api/v1/workspaces/${wsId}/projects`, {
    data: { key: "TCL", name: "Editor lazy boundary", visibility: "workspace" },
  });
  expect(project.status()).toBe(201);
  const projectId = (await readJson(project, flowSchemas.project)).id;
  const task = await page.request.post(`/api/v1/workspaces/${wsId}/projects/${projectId}/tasks`, {
    data: { title: "Lazy route witness", startDate: "2026-09-28", dueDate: "2026-09-30" },
  });
  expect(task.status()).toBe(201);
  await page.goto(`/w/${admin.workspaceSlug}/TCL/gantt?y=2026&m=9`);
  await expect(page.locator('[data-slot="gantt"]')).toBeVisible();
  await captureAssets();
  // The group serves an immutable copied build. Read the actual requested
  // assets after navigation, as the existing Gantt boundary regression does.
  const loaded = await Promise.all(
    [...assets].map(async (path) => {
      const response = await page.request.get(path);
      expect(response.ok()).toBe(true);
      return { path, text: await response.text() };
    }),
  );
  const editorAssets = loaded.filter(({ text }) =>
    /ProseMirror|fvoci-editor|fvociSlash|fvociMention/.test(text),
  );
  expect(editorAssets.map(({ path }) => path)).toEqual([]);
  await expect(editorOf(page)).toHaveCount(0);
});

test("fixed insert and history use the existing room, selection and persisted document", async ({
  browser,
  baseURL,
  page,
}) => {
  const csp = watchCspViolations(page);
  const iconRequests = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "템플릿 도구", { markdown: "시작\n\n동료\n" });
  const peer = await newSignedInPage(browser, baseURL, member);
  try {
    await openDoc(page, doc.path);
    await openDoc(peer.page, doc.path);
    const plugins = await editorOf(page).evaluate((root) =>
      (
        root as HTMLElement & {
          editor: {
            state: {
              plugins: Array<{
                key: string;
              }>;
            };
          };
        }
      ).editor.state.plugins.map((plugin) => plugin.key),
    );
    expect(plugins.filter((key) => key.startsWith("bubbleMenu$"))).toHaveLength(1);
    expect(plugins.filter((key) => key.startsWith("dragHandle$"))).toHaveLength(1);
    const toolbar = page.locator(".fvoci-template-toolbar--fixed");
    await expect(toolbar.locator('[role="group"]')).toHaveCount(2);
    await expect(toolbar.getByRole("button", { name: "실행 취소", exact: true })).toBeDisabled();
    await caretAtEndOf(page, 0);
    await page.keyboard.type(" 내편집");
    await expectBlocks(peer.page, ["시작 내편집", "동료"]);
    await caretAtEndOf(peer.page, 1);
    await peer.page.keyboard.type(" 원격편집");
    await expectBlocks(page, ["시작 내편집", "동료 원격편집"]);
    await toolbar.getByRole("button", { name: "실행 취소", exact: true }).click();
    await expectBlocks(page, ["시작", "동료 원격편집"]);
    await expectBlocks(peer.page, ["시작", "동료 원격편집"]);
    await toolbar.getByRole("button", { name: "다시 실행", exact: true }).click();
    await expectBlocks(peer.page, ["시작 내편집", "동료 원격편집"]);

    await caretAtEndOf(page, 1);
    await toolbar.getByRole("button", { name: "삽입", exact: true }).click();
    const insertMenu = page.getByRole("menu", { name: "삽입", exact: true });
    await expect(insertMenu.getByRole("menuitem").first()).toBeFocused();
    await insertMenu.getByRole("menuitem").first().click();
    const slash = page.locator(".fvoci-suggestion");
    await expect(slash).toBeVisible();
    await expect(slash.getByRole("group", { name: "블록 유형" }).first()).toBeVisible();
    await page.keyboard.type("math");
    await slash.getByRole("option", { name: "수식", exact: true }).click();
    await expect(editorOf(peer.page).locator(".afn-math")).toHaveCount(1);
    await save(page);
    const json = JSON.stringify(await savedBody(page.request, wsId, doc.id));
    expect(json).toContain('"type":"math"');
    expect(json).toContain("원격편집");
    await page.reload();
    await expect(page.locator('[data-collab-status="connected"]')).toBeVisible();
    await expect(editorOf(page).locator(".afn-math")).toHaveCount(1);
    expect(csp).toEqual([]);
    expect(iconRequests).toEqual([]);
  } finally {
    await peer.context.close();
  }
});

test("link popup keeps native selection and composing Enter cannot apply the URL", async ({
  page,
  browser,
  baseURL,
}, testInfo) => {
  await observeTemplateSelection(page);
  const checkpoint = async (stage: string) =>
    page.evaluate((value) => {
      (window as ObservedWindow).__w3TemplateObserver?.checkpoint(value);
    }, stage);
  const caretBoundaries: unknown[] = [];
  const liveBody = async (client: Page): Promise<TiptapNode> =>
    editorOf(client).evaluate(
      (root) =>
        (root as HTMLElement & { editor: Editor }).editor.view.state.doc.toJSON() as TiptapNode,
    );
  const restrictedDbBody = (workspace: string, document: string): unknown => {
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
      version: expect.any(Number),
    });
    return witness;
  };
  const observeCaretBoundary = async (stage: "after-click" | "after-End") => {
    caretBoundaries.push(
      await page.evaluate((value) => {
        const ownerRecordStage = `caret:${value}`;
        (window as ObservedWindow).__w3TemplateObserver?.checkpoint(ownerRecordStage);
        const root = document.querySelector<HTMLElement & { editor?: Editor }>(
          ".fvoci-editor .ProseMirror",
        );
        const editor = root?.editor;
        const view = editor && !editor.isDestroyed ? editor.view : undefined;
        const native = window.getSelection();
        const endpoint = (node: Node | null | undefined, offset: number | undefined) => {
          const inside = Boolean(node && root?.contains(node));
          const element = node instanceof Element ? node : node?.parentElement;
          const leaf = element?.closest('[contenteditable="false"]');
          let position: number | null = null;
          if (inside && node && offset !== undefined && view) {
            try {
              position = view.posAtDOM(node, offset);
            } catch {
              // A detached or unmappable endpoint stays unknown; do not repair it.
            }
          }
          return {
            inside,
            noneditableLeaf: Boolean(leaf && leaf !== root && root?.contains(leaf)),
            position,
          };
        };
        const selection = view?.state.selection;
        const mode = root?.closest<HTMLElement>(".fvoci-editor")?.dataset.editorModeActive;
        return {
          stage: value,
          ownerRecordStage,
          at: performance.now(),
          rootPresent: Boolean(root),
          editorDestroyed: editor?.isDestroyed ?? null,
          native: {
            collapsed: native?.isCollapsed ?? null,
            text: native?.toString() ?? null,
            anchor: endpoint(native?.anchorNode, native?.anchorOffset),
            head: endpoint(native?.focusNode, native?.focusOffset),
          },
          focus: {
            activeTag: document.activeElement?.tagName ?? null,
            editor: view?.hasFocus() ?? null,
            editable: view?.editable ?? null,
            domEditable: root?.contentEditable ?? null,
            composing: view?.composing ?? null,
          },
          wide: !window.matchMedia("(max-width: 47.999rem)").matches,
          rich: mode === "rich" || mode === "block",
          pm: selection
            ? {
                anchor: selection.anchor,
                head: selection.head,
                empty: selection.empty,
                type: (selection.toJSON() as { type?: unknown }).type,
              }
            : null,
        };
      }, stage),
    );
  };
  try {
    const csp = watchCspViolations(page);
    await login(page, admin.email, admin.password);
    const wsId = await workspaceId(page.request);
    const doc = await createDoc(page.request, wsId, "템플릿 링크", {
      markdown: "한글과 😀 링크\n",
    });
    const originalStored = await savedBody(page.request, wsId, doc.id);
    expect(originalStored).toEqual({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "한글과 😀 링크" }] }],
    });
    await openDoc(page, doc.path);
    await checkpoint("openDoc:original");
    const originalLive = await liveBody(page);
    const liveAttributes = originalLive.content?.[0]?.attrs;
    if (!liveAttributes || typeof liveAttributes.id !== "string" || !liveAttributes.id)
      throw new Error("Missing original mounted paragraph identity");
    const originalMountedID = liveAttributes.id;
    const bareContent = [
      { type: "text", text: "한글과 " },
      { type: "emoji", attrs: { name: "grinning" } },
      { type: "text", text: " 링크" },
    ];
    expect(originalLive).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: originalMountedID, ychange: null, textAlign: null },
          content: bareContent,
        },
      ],
    });
    await blockAt(page, 0).click();
    await observeCaretBoundary("after-click");
    await page.keyboard.press("End");
    await observeCaretBoundary("after-End");
    await page.keyboard.press("Shift+Home");
    await checkpoint("ShiftHome:original-return");
    const bubble = page.locator("[data-fvoci-bubble]");
    const trigger = bubble.getByRole("button", { name: "링크", exact: true });
    await expect(bubble).toBeVisible();
    await checkpoint("bubble:original-visible");
    await trigger.click();
    const dialog = page.getByRole("dialog", { name: "링크", exact: true });
    const url = dialog.getByLabel("URL");
    await expect(url).toBeFocused();
    await checkpoint("popup:original-open-focus");
    await url.fill("https://example.com/한글");
    await url.dispatchEvent("compositionstart", { data: "한" });
    await url.dispatchEvent("keydown", { key: "Enter", isComposing: true, keyCode: 229 });
    await expect(dialog).toBeVisible();
    await expect(blockAt(page, 0).locator("a")).toHaveCount(0);
    await checkpoint("compositionEnter:original-no-link");
    const composingBody = await liveBody(page);
    await url.dispatchEvent("compositionend", { data: "한글" });
    await page.keyboard.press("Escape");
    await expect(trigger).toBeFocused();
    expect(await editorOf(page).evaluate(() => window.getSelection()?.toString())).toBe(
      "한글과 😀 링크",
    );
    await checkpoint("Cancel:original-native-text");
    const cancelledBody = await liveBody(page);
    await trigger.click();
    await url.fill("https://example.com/한글");
    await url.press("Enter");
    await expect(dialog).toHaveCount(0);
    await expect(blockAt(page, 0).locator("a")).toHaveText("한글과 😀 링크");
    await checkpoint("Apply:original-selected-text");
    await save(page);
    expect(JSON.stringify(await savedBody(page.request, wsId, doc.id))).toContain('"type":"link"');
    expect(csp).toEqual([]);
    await checkpoint("save:original");
    // The unchanged MarkedEmoji foundation policy encodes a marked known
    // emoji as its exact Unicode glyph plus marks; bare atoms keep their name.
    const linkedContent = [
      {
        type: "text",
        text: "한글과 😀 링크",
        marks: [
          {
            type: "link",
            attrs: {
              href: "https://example.com/한글",
              target: "_blank",
              rel: "noopener noreferrer nofollow",
              class: null,
              title: null,
            },
          },
        ],
      },
    ];
    const expectedStored: TiptapNode = {
      type: "doc",
      // The seed/projection contract omits default-null paragraph attrs.
      // Identity comes from the original mounted snapshot, before Apply.
      content: [{ type: "paragraph", attrs: { id: originalMountedID }, content: linkedContent }],
    };
    const expectedLive: TiptapNode = {
      type: "doc",
      content: [{ type: "paragraph", attrs: { ...liveAttributes }, content: linkedContent }],
    };
    expect(composingBody).toEqual(originalLive);
    expect(cancelledBody).toEqual(originalLive);
    expect(await savedBody(page.request, wsId, doc.id)).toEqual(expectedStored);
    expect(await liveBody(page)).toEqual(expectedLive);
    expect(
      await editorOf(page).evaluate((root) => {
        const selection = (root as HTMLElement & { editor: Editor }).editor.view.state.selection;
        return { anchor: selection.anchor, head: selection.head, empty: selection.empty };
      }),
    ).toEqual({ anchor: 10, head: 1, empty: false });
    await expect(blockAt(page, 0).locator("a")).toHaveAttribute("href", "https://example.com/한글");
    const dbWitness = restrictedDbBody(wsId, doc.id);
    await testInfo.attach("w3-template-link-db-witness.json", {
      body: JSON.stringify(dbWitness),
      contentType: "application/json",
    });
    if (typeof dbWitness !== "object" || dbWitness === null || !("content" in dbWitness))
      throw new Error("Missing restricted DB body");
    expect(dbWitness.content).toEqual(expectedStored);
    const fresh = await newSignedInPage(browser, baseURL, admin);
    try {
      await openDoc(fresh.page, doc.path);
      expect(await liveBody(fresh.page)).toEqual(expectedLive);
      expect(await savedBody(fresh.page.request, wsId, doc.id)).toEqual(expectedStored);
      await expect(blockAt(fresh.page, 0).locator("a")).toHaveCount(1);
      await expect(blockAt(fresh.page, 0).locator("a")).toHaveText("한글과 😀 링크");
      await expect(blockAt(fresh.page, 0).locator("a")).toHaveAttribute(
        "href",
        "https://example.com/한글",
      );
    } finally {
      await fresh.context.close();
    }
  } finally {
    const observation = await page
      .evaluate(() => {
        const current = (window as ObservedWindow).__w3TemplateObserver;
        if (!current) return { unavailable: "No installed observer in current page" };
        const result = current.stop();
        delete (window as ObservedWindow).__w3TemplateObserver;
        return result;
      })
      .catch((error: unknown) => ({ unavailable: String(error) }));
    await testInfo.attach("w3-template-native-selection-observation.json", {
      body: JSON.stringify({
        ...(typeof observation === "object" && observation !== null ? observation : {}),
        caretBoundaries,
      }),
      contentType: "application/json",
    });
  }
});

test("emoji insertion and mobile groups remain keyboard usable without viewport overflow", async ({
  page,
}) => {
  const iconRequests = watchIconRequests(page);
  await login(page, admin.email, admin.password);
  const wsId = await workspaceId(page.request);
  const doc = await createDoc(page.request, wsId, "템플릿 이모지", { markdown: "문단\n" });
  await openDoc(page, doc.path);
  await caretAtEndOf(page, 0);
  await page
    .locator(".fvoci-template-toolbar--fixed")
    .getByRole("button", { name: "삽입", exact: true })
    .click();
  await page
    .getByRole("menu", { name: "삽입", exact: true })
    .getByRole("menuitem", { name: ":", exact: true })
    .click();
  await page.keyboard.type("smile");
  const suggestions = page.locator(".fvoci-suggestion");
  await expect(suggestions).toBeVisible();
  await expect(suggestions.getByRole("option").first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowDown");
  await expect(suggestions.getByRole("option").nth(1)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(suggestions).toHaveCount(0);
  await save(page);
  expect(JSON.stringify(await savedBody(page.request, wsId, doc.id))).toContain('"type":"emoji"');
  await page.setViewportSize({ width: 390, height: 844 });
  const mobile = page.locator("[data-mobile-toolbar]");
  await expect(mobile).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  const bold = mobile.getByRole("button", { name: "굵게", exact: true });
  const box = await bold.boundingBox();
  expect(box?.height).toBeGreaterThanOrEqual(44);
  await caretAtEndOf(page, 0);
  await page.keyboard.press("Shift+Home");
  await bold.click();
  await expect(blockAt(page, 0).locator("strong")).toContainText("문단");
  await expect(page.locator("[data-fvoci-bubble]")).toBeHidden();
  expect(iconRequests).toEqual([]);
});
