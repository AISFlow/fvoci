import assert from "node:assert/strict";
import test from "node:test";
import {
  nativeOwnedDeleteDecision,
  type NativeOwnedDeleteInput,
} from "../src/react/overlay-owner.ts";

const base = (
  overrides: Partial<NativeOwnedDeleteInput> = {},
): NativeOwnedDeleteInput => ({
  editable: true,
  trusted: true,
  composing: false,
  keyCode: 46,
  key: "Delete",
  pmIsTextSelection: true,
  pmAnchor: 6,
  pmHead: 6,
  nativeAnchorInside: true,
  nativeFocusInside: true,
  nativeAnchorPos: 3,
  nativeFocusPos: 3,
  ...overrides,
});

test("native owner aligns Delete when collapsed native pos differs from PM", () => {
  assert.deepEqual(nativeOwnedDeleteDecision(base()), {
    take: true,
    anchorPos: 3,
    headPos: 3,
  });
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ key: "Backspace", keyCode: 8 })),
    { take: true, anchorPos: 3, headPos: 3 },
  );
});

test("native owner falls through when PM already matches native caret", () => {
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ nativeAnchorPos: 3, nativeFocusPos: 3, pmAnchor: 3, pmHead: 3 })),
    { take: false },
  );
});

test("native owner aligns when PM range disagrees with native range", () => {
  assert.deepEqual(
    nativeOwnedDeleteDecision(
      base({
        nativeAnchorPos: 1,
        nativeFocusPos: 4,
        pmAnchor: 6,
        pmHead: 6,
      }),
    ),
    { take: true, anchorPos: 1, headPos: 4 },
  );
});

test("native owner skips untrusted, IME, readonly, non-text PM, and keys outside Delete/Backspace", () => {
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ trusted: false })),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ composing: true })),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ keyCode: 229 })),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ editable: false })),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ pmIsTextSelection: false })),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(
      base({ nativeAnchorInside: false, nativeAnchorPos: null }),
    ),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(
      base({ nativeFocusInside: false, nativeFocusPos: null }),
    ),
    { take: false },
  );
  assert.deepEqual(
    nativeOwnedDeleteDecision(base({ key: "Enter", keyCode: 13 })),
    { take: false },
  );
});
