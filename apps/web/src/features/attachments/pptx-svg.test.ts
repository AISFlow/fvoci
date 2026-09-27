import assert from "node:assert/strict";
import test from "node:test";
import { neutralizeSvgUrls } from "./pptx-svg.ts";

test("fragment and embedded data references survive; everything else becomes none", () => {
  assert.equal(neutralizeSvgUrls("url(#clip-1)"), "url(#clip-1)");
  assert.equal(neutralizeSvgUrls('fill:url("#grad")'), 'fill:url("#grad")');
  assert.equal(neutralizeSvgUrls("url(data:image/png;base64,AAAA)"), "url(data:image/png;base64,AAAA)");
  assert.equal(neutralizeSvgUrls("url('data:font/woff2;base64,AAAA')"), "url('data:font/woff2;base64,AAAA')");
  for (const target of [
    "https://example.com/x.png",
    "//example.com/x",
    "/api/v1/me",
    "other.svg#frag",
    "javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "# spaced",
  ]) {
    // An unquoted CSS url() cannot contain parentheses; only quoted forms are tested for those.
    if (!target.includes("(")) assert.equal(neutralizeSvgUrls(`background:url(${target})`), "background:none", target);
    assert.equal(neutralizeSvgUrls(`background:url("${target}")`), "background:none", target);
  }
  assert.equal(neutralizeSvgUrls("@import url(https://example.com/a.css); fill:red"), " fill:red");
  assert.equal(
    neutralizeSvgUrls("font-family:X;background:url(https://example.com/beacon)"),
    "font-family:X;background:none",
  );
});
