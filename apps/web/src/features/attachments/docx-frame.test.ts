import assert from "node:assert/strict";
import test from "node:test";
import { DOCX_FRAME_CSP, isSafeDataImageUrl, neutralizeCssUrls } from "./docx-frame.ts";

test("the frame CSP allows no script, no network and only data: images and fonts", () => {
  assert.match(DOCX_FRAME_CSP, /default-src 'none'/);
  assert.match(DOCX_FRAME_CSP, /script-src 'none'/);
  assert.match(DOCX_FRAME_CSP, /img-src data:;/);
  assert.match(DOCX_FRAME_CSP, /font-src data:;/);
  assert.doesNotMatch(DOCX_FRAME_CSP, /'self'|https?:|blob:/);
});

test("only embedded data: images are kept as resource URLs", () => {
  assert.equal(isSafeDataImageUrl("data:image/png;base64,AAAA"), true);
  assert.equal(isSafeDataImageUrl(" data:image/svg+xml;base64,AAAA"), true);
  for (const url of [
    "https://example.com/a.png",
    "//example.com/a.png",
    "/api/v1/me",
    "javascript:alert(1)",
    "data:text/html;base64,PHNjcmlwdD4=",
    "blob:https://app/x",
    "null",
  ]) {
    assert.equal(isSafeDataImageUrl(url), false, url);
  }
});

test("CSS url() targets other than embedded images or fonts become none", () => {
  const css = [
    "@import url(https://evil.example/x.css);",
    ".a{background:url(https://evil.example/p.png)}",
    ".b{background:url('/api/v1/me')}",
    '.c{background:url("data:image/png;base64,AAAA")}',
    "@font-face{font-family:X;src:url(data:font/woff2;base64,AAAA)}",
    ".d{list-style-image:url( //evil.example/i.gif )}",
  ].join("\n");
  const out = neutralizeCssUrls(css);
  assert.doesNotMatch(out, /evil\.example|\/api\/v1|@import/);
  assert.match(out, /url\("data:image\/png;base64,AAAA"\)/);
  assert.match(out, /url\(data:font\/woff2;base64,AAAA\)/);
  assert.equal((out.match(/:none/g) ?? []).length, 3);
});
