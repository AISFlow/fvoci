import assert from "node:assert/strict";
import test from "node:test";
import { restoreNativeSelection } from "../src/vue/menu.ts";

test("selection failures restore the original contenteditable attribute", () => {
  for (const original of [null, "true", "false"]) {
    for (const failing of ["removeAllRanges", "addRange"]) {
      let editable = original;
      const dom = {
        getAttribute: () => editable,
        setAttribute: (_name: string, value: string) => {
          editable = value;
        },
        removeAttribute: () => {
          editable = null;
        },
        set contentEditable(value: string) {
          editable = value;
        },
        ownerDocument: {
          createRange: () => ({ setStart() {}, setEnd() {} }),
          getSelection: () => ({
            removeAllRanges() {
              if (failing === "removeAllRanges") throw new Error("detached selection");
            },
            addRange() {
              if (failing === "addRange") throw new Error("detached selection");
            },
          }),
        },
      };
      restoreNativeSelection({
        isDestroyed: false,
        state: { selection: { from: 1, to: 2 } },
        view: {
          dom: dom as unknown as HTMLElement,
          domAtPos: () => ({ node: {} as Node, offset: 0 }),
        },
      });
      assert.equal(editable, original, `${failing}, original=${original}`);
    }
  }
});
