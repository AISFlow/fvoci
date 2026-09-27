import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import {
  RHWP_CORE_VERSION,
  RHWP_THIRD_PARTY_FILE,
  RHWP_THIRD_PARTY_SHA256,
  rhwpWasmNoticeTitle,
} from "./rhwp-notice.ts";

const licensesRoot = path.resolve(import.meta.dirname, "../../../../../third-party/browser-licenses");

test("the rhwp third-party table is the pinned upstream copy", () => {
  const bytes = fs.readFileSync(path.join(licensesRoot, RHWP_THIRD_PARTY_FILE));
  assert.equal(createHash("sha256").update(bytes).digest("hex"), RHWP_THIRD_PARTY_SHA256);
  assert.match(bytes.toString("utf8"), /`rhwp` v0\.8\.6/);
});

test("the notice follows the installed @rhwp/core version", () => {
  const require = createRequire(import.meta.url);
  const pkg = JSON.parse(
    fs.readFileSync(path.join(path.dirname(require.resolve("@rhwp/core")), "package.json"), "utf8"),
  ) as { version: string };
  assert.equal(pkg.version, RHWP_CORE_VERSION);
  assert.match(rhwpWasmNoticeTitle(pkg.version), /e8800c8def63449808a4092798442652ed460552/);
  assert.throws(() => rhwpWasmNoticeTitle("0.8.7"), /update the rhwp wasm notice/);
});
