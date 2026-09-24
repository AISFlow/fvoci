import assert from "node:assert/strict";
import test from "node:test";
import { formatBytes } from "../src/react/format-bytes.ts";

test("formatBytes 는 1024 단위로 올리고 100 미만이면 소수 한 자리", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(999), "999 B");
  assert.equal(formatBytes(1536), "1.5 KB");
  assert.equal(formatBytes(1_258_291), "1.2 MB");
  assert.equal(formatBytes(104_857_600), "100 MB");
  assert.equal(formatBytes(-1), "");
});
