import assert from "node:assert/strict";
import test from "node:test";
import {
  isNativeOwnedDeleteKey,
  type NativeDeleteKey,
} from "../src/react/native-delete-owner.ts";

const base = (overrides: Partial<NativeDeleteKey> = {}): NativeDeleteKey => ({
  trusted: true,
  editable: true,
  composing: false,
  keyCode: 46,
  key: "Delete",
  pmIsTextSelection: true,
  ...overrides,
});

test("trusted Delete and Backspace on a TextSelection are eligible", () => {
  assert.equal(isNativeOwnedDeleteKey(base()), true);
  assert.equal(isNativeOwnedDeleteKey(base({ key: "Backspace", keyCode: 8 })), true);
});

test("untrusted, readonly, composing and IME keydowns are left to PM", () => {
  assert.equal(isNativeOwnedDeleteKey(base({ trusted: false })), false);
  assert.equal(isNativeOwnedDeleteKey(base({ editable: false })), false);
  assert.equal(isNativeOwnedDeleteKey(base({ composing: true })), false);
  assert.equal(isNativeOwnedDeleteKey(base({ keyCode: 229 })), false);
});

test("other keys and non-text selections are left to PM", () => {
  assert.equal(isNativeOwnedDeleteKey(base({ key: "ArrowLeft", keyCode: 37 })), false);
  assert.equal(isNativeOwnedDeleteKey(base({ key: "a", keyCode: 65 })), false);
  assert.equal(isNativeOwnedDeleteKey(base({ pmIsTextSelection: false })), false);
});
