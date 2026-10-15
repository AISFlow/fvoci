// Shared real-keyboard oracle; observes native selection before keyup and focus repair.
import { expect, type Page, type TestInfo } from "@playwright/test";
import type { Editor } from "@tiptap/core";
import { caretAtEndOf, editorOf } from "./workspace-wiki-vue-editor";

export async function expectDocumentBoundaries(page: Page, info: TestInfo): Promise<void> {
  for (const key of ["Home", "End"]) {
    for (const shift of [false, true]) {
      await caretAtEndOf(page, 1);
      const initial = await editorOf(page).evaluate((element) => {
        const root = element as HTMLElement & { editor: Editor };
        const snapshot = () => {
          const native = document.getSelection();
          if (!native?.anchorNode || !native.focusNode) throw new Error("missing DOM selection");
          const { view, state } = root.editor;
          return {
            anchor: state.selection.anchor,
            head: state.selection.head,
            nativeAnchor: view.posAtDOM(native.anchorNode, native.anchorOffset),
            nativeHead: view.posAtDOM(native.focusNode, native.focusOffset),
          };
        };
        const nativeTimeout = window.setTimeout.bind(window);
        let focusing = false;
        let repair: (() => void) | undefined;
        let timer: number | undefined;
        root.addEventListener("focus", () => (focusing = true), { capture: true, once: true });
        root.addEventListener("focusin", () => (focusing = false), { once: true });
        window.setTimeout = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
          // Capture the real prosemirror-view focus callback, as the line-boundary
          // regression below does. Deliver it after native keydown, before keyup.
          if (focusing && delay === 20 && typeof callback === "function") {
            if (repair) throw new Error("multiple editor focus-repair tasks");
            repair = () => {
              Reflect.apply(callback, window, args);
            };
            timer = nativeTimeout(() => {}, delay);
            return timer;
          }
          return nativeTimeout(callback, delay, ...args);
        }) as typeof window.setTimeout;
        try {
          root.blur();
          root.focus();
        } finally {
          window.setTimeout = nativeTimeout;
          window.clearTimeout(timer);
        }
        if (!repair) throw new Error("missing pending editor focus repair");
        const deliver = repair;
        // Hold selectionchange until the repair has run: this is the ordering
        // that loses the native caret, independent of browser task scheduling.
        const holdSelectionChange = (event: Event) => {
          event.stopImmediatePropagation();
        };
        document.addEventListener("selectionchange", holdSelectionChange, true);
        Object.assign(window, {
          __fvociDocumentBoundaryRepair: () => {
            try {
              const before = snapshot();
              deliver();
              return { before, after: snapshot() };
            } finally {
              document.removeEventListener("selectionchange", holdSelectionChange, true);
            }
          },
        });
        return { ...snapshot(), end: root.editor.state.doc.content.size - 1 };
      });
      expect(initial.nativeHead).toBe(initial.head);
      await page.keyboard.down("Control");
      if (shift) await page.keyboard.down("Shift");
      try {
        await page.keyboard.down(key);
        const witness = await page.evaluate(() => {
          const holder = window as unknown as {
            __fvociDocumentBoundaryRepair?: () => {
              before: { anchor: number; head: number; nativeAnchor: number; nativeHead: number };
              after: { anchor: number; head: number; nativeAnchor: number; nativeHead: number };
            };
          };
          const deliver = holder.__fvociDocumentBoundaryRepair;
          delete holder.__fvociDocumentBoundaryRepair;
          if (!deliver) throw new Error("missing document boundary witness");
          return deliver();
        });
        await info.attach(`Ctrl${shift ? "-Shift" : ""}-${key}-before-keyup`, {
          body: JSON.stringify(witness),
          contentType: "application/json",
        });
        const head = key === "Home" ? 1 : initial.end;
        const anchor = shift ? initial.anchor : head;
        for (const stage of [witness.before, witness.after]) {
          expect(stage).toEqual({ anchor, head, nativeAnchor: anchor, nativeHead: head });
        }
      } finally {
        await page.keyboard.up(key);
        if (shift) await page.keyboard.up("Shift");
        await page.keyboard.up("Control");
      }
    }
  }
}
