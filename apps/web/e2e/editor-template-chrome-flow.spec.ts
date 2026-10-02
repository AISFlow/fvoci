// Official Nuxt UI editor chrome on FVOCI's existing Tiptap/Yjs host.
// Real browser, Rust server, DB, peer and persisted ACK; no demo document/store.
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
    let firstOwner: string | undefined;
    let previousNativeId: unknown;
    let previousBubble = false;
    let updates = 0;
    let localUpdates = 0;
    let stopped = false;
    let animation = 0;
    const capture = (stage: string, retain = false) => {
      try {
        const root = document.querySelector<HTMLElement & { editor?: Editor }>(
          ".fvoci-editor .ProseMirror",
        );
        const current = root?.editor;
        if (!current || current.isDestroyed) return;
        if (!bound) {
          bound = current;
          const options = current.extensionManager.extensions.find(
            (extension) => extension.name === "collaboration",
          )?.options as { document?: Y.Doc } | undefined;
          ydoc = options?.document;
          provider = (
            current.extensionManager.extensions.find(
              (extension) => extension.name === "collaborationCaret",
            )?.options as { provider?: HocuspocusProvider } | undefined
          )?.provider;
          bound.on("transaction", transaction);
          ydoc?.on("update", update);
          provider?.on("authenticated", authenticated);
          provider?.on("status", status);
          provider?.on("synced", synced);
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
        // Read an existing shared type; do not create or seed one for observation.
        const fragment = ydoc?.share.get("prosemirror") as Y.XmlFragment | undefined;
        const node = fragment?.toArray()[0];
        const nativeId: unknown = node && "getAttribute" in node ? node.getAttribute("id") : null;
        const owner = JSON.stringify([
          token(root),
          token(current),
          token(current.view.dom),
          token(ydoc),
          token(provider),
          token(fragment),
          token(node),
        ]);
        if (!firstOwner) firstOwner = owner;
        if (owner !== firstOwner) ownerChanges.push({ stage, at: performance.now(), owner });
        const bubble = document.querySelector<HTMLElement>("[data-fvoci-bubble]");
        const bubbleChanged = Boolean(bubble) !== previousBubble;
        const idChanged = nativeId !== previousNativeId;
        previousBubble = Boolean(bubble);
        previousNativeId = nativeId;
        const frame = {
          at: performance.now(),
          stage,
          owner,
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
            type: state.selection.constructor.name,
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
        frames.push(frame);
        if (frames.length > 512) frames.shift();
        if (retain || bubbleChanged || idChanged) critical.push(frame);
        if (
          nativePositions &&
          "anchor" in nativePositions &&
          (nativePositions.anchor !== state.selection.anchor ||
            nativePositions.head !== state.selection.head)
        ) {
          // Native-to-PM settling is observed, not classified as a defect here.
          observedMismatches.push({ at: frame.at, stage, nativePositions, pm: frame.pm });
        }
      } catch (error) {
        critical.push({ at: performance.now(), stage, unknown: String(error) });
      }
    };
    const transaction = ({ transaction: tr }: { transaction: Transaction }) => {
      capture(
        `transaction:doc=${String(tr.docChanged)}:selection=${String(tr.selectionSet)}`,
        tr.docChanged,
      );
    };
    const update = (_bytes: Uint8Array, _origin: unknown, _doc: Y.Doc, tr: Y.Transaction) => {
      updates++;
      if (tr.local) localUpdates++;
      capture(`Yupdate:local=${String(tr.local)}`, true);
    };
    const authenticated = () => {
      capture("provider:authenticated", true);
    };
    const status = () => {
      capture("provider:status", true);
    };
    const synced = () => {
      capture("provider:synced", true);
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
    const tick = () => {
      if (stopped) return;
      capture("frame");
      animation = requestAnimationFrame(tick);
    };
    animation = requestAnimationFrame(tick);
    (window as ObservedWindow).__w3TemplateObserver = {
      checkpoint: (stage) => {
        capture(stage, true);
      },
      stop() {
        capture("finally", true);
        stopped = true;
        cancelAnimationFrame(animation);
        for (const name of events) document.removeEventListener(name, event, true);
        bound?.off("transaction", transaction);
        ydoc?.off("update", update);
        provider?.off("authenticated", authenticated);
        provider?.off("status", status);
        provider?.off("synced", synced);
        return { frames, critical, ownerChanges, observedMismatches, updates, localUpdates };
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
}, testInfo) => {
  await observeTemplateSelection(page);
  const checkpoint = async (stage: string) =>
    page.evaluate((value) => {
      (window as ObservedWindow).__w3TemplateObserver?.checkpoint(value);
    }, stage);
  try {
    const csp = watchCspViolations(page);
    await login(page, admin.email, admin.password);
    const wsId = await workspaceId(page.request);
    const doc = await createDoc(page.request, wsId, "템플릿 링크", {
      markdown: "한글과 😀 링크\n",
    });
    await openDoc(page, doc.path);
    await checkpoint("openDoc:original");
    await caretAtEndOf(page, 0);
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
    await url.dispatchEvent("compositionend", { data: "한글" });
    await page.keyboard.press("Escape");
    await expect(trigger).toBeFocused();
    expect(await editorOf(page).evaluate(() => window.getSelection()?.toString())).toBe(
      "한글과 😀 링크",
    );
    await checkpoint("Cancel:original-native-text");
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
      body: JSON.stringify(observation),
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
