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
  RHWP_WASM_CRATE_COUNT,
  rhwpWasmNoticeTitle,
} from "./rhwp-notice.ts";

const licensesRoot = path.resolve(import.meta.dirname, "../../../../../third-party/browser-licenses");

test("the rhwp wasm notice is the pinned wasm32 graph concatenation", () => {
  const bytes = fs.readFileSync(path.join(licensesRoot, RHWP_THIRD_PARTY_FILE));
  assert.equal(createHash("sha256").update(bytes).digest("hex"), RHWP_THIRD_PARTY_SHA256);
  const text = bytes.toString("utf8");
  assert.match(text, new RegExp(`^${RHWP_WASM_CRATE_COUNT} crates:$`, "m"));
  assert.equal(text.match(/^ {2}\S+ \d\S*: /gm)?.length, RHWP_WASM_CRATE_COUNT);
  // AND-licensed and incorporated texts a crate table alone would not carry.
  assert.match(text, /^Applies to: encoding_rs 0\.8\.35 \(LICENSE-WHATWG\)$/m);
  assert.match(text, /^Applies to: unicode-ident 1\.0\.24 \(LICENSE-UNICODE\)$/m);
  assert.match(text, /Copyright © `2024`, `Volexity, Inc`/);
  // Native-only or non-default rhwp dependencies are not in the wasm.
  for (const name of ["svg2pdf", "usvg", "pdf-writer", "subsetter", "resvg", "skia-safe", "vello", "gif", "libc", "r-efi", "cc"]) {
    assert.doesNotMatch(text, new RegExp(`^ {2}${name} `, "m"), name);
  }
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
