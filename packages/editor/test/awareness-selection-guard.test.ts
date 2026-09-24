import assert from "node:assert/strict";
import test from "node:test";
import {
  shouldAdoptNativeOnAwareness,
  type AwarenessSelectionGuardInput,
} from "../src/react/awareness-selection-guard.ts";

const base = (
  overrides: Partial<AwarenessSelectionGuardInput> = {},
): AwarenessSelectionGuardInput => ({
  awarenessUpdated: true,
  docChanged: false,
  selectionSet: false,
  composing: false,
  editable: true,
  pmIsTextSelection: true,
  observedDomSelectionMatchesNative: false,
  ...overrides,
});

test("awareness-only decoration transactions adopt a native caret ahead of PM", () => {
  assert.equal(shouldAdoptNativeOnAwareness(base()), true);
});

test("matching observed DOM selection is not treated as native-ahead", () => {
  assert.equal(
    shouldAdoptNativeOnAwareness(base({ observedDomSelectionMatchesNative: true })),
    false,
  );
});

test("doc or selection changes keep the existing PM path", () => {
  assert.equal(shouldAdoptNativeOnAwareness(base({ docChanged: true })), false);
  assert.equal(shouldAdoptNativeOnAwareness(base({ selectionSet: true })), false);
});

test("unrelated transactions, IME, readonly and non-text selections are skipped", () => {
  assert.equal(shouldAdoptNativeOnAwareness(base({ awarenessUpdated: false })), false);
  assert.equal(shouldAdoptNativeOnAwareness(base({ composing: true })), false);
  assert.equal(shouldAdoptNativeOnAwareness(base({ editable: false })), false);
  assert.equal(shouldAdoptNativeOnAwareness(base({ pmIsTextSelection: false })), false);
});
