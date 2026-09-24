import assert from "node:assert/strict";
import test from "node:test";
import {
  shouldAdoptNativeOnAwareness,
  shouldAdoptNativeOnSelectionChange,
  shouldWriteSelectionToDom,
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
  ...overrides,
});

test("awareness-only decoration transactions adopt the native caret", () => {
  assert.equal(shouldAdoptNativeOnAwareness(base()), true);
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

test("selectionchange adopts native when the caret is inside an editable editor", () => {
  assert.equal(
    shouldAdoptNativeOnSelectionChange({
      composing: false,
      editable: true,
      nativeInside: true,
      pmIsTextSelection: true,
    }),
    true,
  );
});

test("selectionchange leaves IME, outside caret, readonly and non-text selections to PM", () => {
  const ok = {
    composing: false,
    editable: true,
    nativeInside: true,
    pmIsTextSelection: true,
  };
  assert.equal(shouldAdoptNativeOnSelectionChange({ ...ok, composing: true }), false);
  assert.equal(shouldAdoptNativeOnSelectionChange({ ...ok, editable: false }), false);
  assert.equal(shouldAdoptNativeOnSelectionChange({ ...ok, nativeInside: false }), false);
  assert.equal(
    shouldAdoptNativeOnSelectionChange({ ...ok, pmIsTextSelection: false }),
    false,
  );
});

test("decoration and focus-timeout writes do not clobber an ahead native caret", () => {
  assert.equal(
    shouldWriteSelectionToDom({
      force: false,
      selectionSet: false,
      native: { from: 5, to: 5 },
      writeFrom: 1,
      writeTo: 1,
    }),
    false,
  );
});

test("selectionSet, forced writes, missing native, ranges, and matching native still update the DOM", () => {
  const clobber = {
    force: false,
    selectionSet: false,
    native: { from: 5, to: 5 } as const,
    writeFrom: 1,
    writeTo: 1,
  };
  assert.equal(shouldWriteSelectionToDom({ ...clobber, force: true }), true);
  assert.equal(shouldWriteSelectionToDom({ ...clobber, selectionSet: true }), true);
  assert.equal(shouldWriteSelectionToDom({ ...clobber, native: null }), true);
  assert.equal(
    shouldWriteSelectionToDom({
      ...clobber,
      native: { from: 1, to: 1 },
    }),
    true,
  );
  assert.equal(
    shouldWriteSelectionToDom({
      force: false,
      selectionSet: false,
      native: { from: 1, to: 5 },
      writeFrom: 1,
      writeTo: 1,
    }),
    true,
  );
});
