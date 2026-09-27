import assert from "node:assert/strict";
import test from "node:test";
import { openPptx, renderSlide, type PptxDeck } from "./pptx-deck.ts";
import { neutralizeSvgUrls, sanitizeSlideSvg } from "./pptx-svg.ts";
import { buildFixturePptx, DEFAULT_PPTX_TEXT, FIXTURE_PPTX_EXTERNAL_LINK } from "./pptx-test-fixture.ts";

async function fixtureSvg(index: number): Promise<string> {
  const opened = await openPptx(buildFixturePptx(), () => true);
  assert.equal(opened.status, "ok");
  const rendered = renderSlide((opened as { deck: PptxDeck }).deck, index);
  assert.equal(rendered.status, "ok");
  return (rendered as { svg: string }).svg;
}

const wrap = (inner: string) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">${inner}</svg>`;

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
});

test("renderer output: only the link tags go; text, styles, shapes and the embedded picture stay", async () => {
  const raw = await fixtureSvg(0);
  const clean = sanitizeSlideSvg(raw);
  assert.ok(clean !== null);
  assert.equal(clean, raw.replace(/<a [^>]*>|<\/a>/g, ""));
  assert.doesNotMatch(clean, /<a[\s>]/);
  assert.ok(!clean.includes(FIXTURE_PPTX_EXTERNAL_LINK));
  assert.ok(!clean.includes("javascript:"));
  assert.match(clean, /<image [^>]*href="data:image\/png;base64,/);
  for (const part of [DEFAULT_PPTX_TEXT.link, DEFAULT_PPTX_TEXT.scriptLink, ...DEFAULT_PPTX_TEXT.table]) {
    assert.ok(clean.includes(part), part);
  }
  // Link styling is on the inner span and survives the unwrap.
  assert.match(clean, /text-decoration:underline;color:#0563C1">외부 링크/);
  const second = await fixtureSvg(1);
  assert.equal(sanitizeSlideSvg(second), second);
});

test("remote references are dropped or neutralized, fragments kept", () => {
  const out = sanitizeSlideSvg(
    wrap(
      '<image href="https://example.com/t.png" xlink:href="//example.com/t.png" width="1"/>' +
        '<image href="data:image/png;base64,AAAA" width="1"/>' +
        '<rect fill="url(#g)" clip-path="url(https://example.com/c.svg#c)" style="font-family:X;background:url(https://example.com/b)"/>' +
        '<div xmlns="http://www.w3.org/1999/xhtml"><img src="https://example.com/i.png"/><img src="data:image/gif;base64,R0"/></div>' +
        "<text>url(https://example.com/in-text) stays text</text>",
    ),
  );
  assert.ok(out !== null);
  assert.ok(!/https:\/\/example\.com\/(t|c|b|i)\b/.test(out), out);
  assert.match(out, /<image width="1"\/>/);
  assert.match(out, /href="data:image\/png;base64,AAAA"/);
  assert.match(out, /fill="url\(#g\)" clip-path="none"/);
  assert.match(out, /background:none/);
  assert.match(out, /<img\/>/);
  assert.match(out, /<img src="data:image\/gif;base64,R0"\/>/);
  assert.match(out, /<text>url\(https:\/\/example\.com\/in-text\) stays text<\/text>/);
});

test("markup the renderer never emits makes the slide unavailable", () => {
  for (const inner of [
    "<script>alert(1)</script>",
    '<rect onclick="alert(1)"/>',
    '<rect ONLOAD = "x"/>',
    '<set attributeName="href" to="javascript:alert(1)"/>',
    '<animate attributeName="x"/>',
    "<style>rect{fill:url(https://example.com/x)}</style>",
    '<foreignObject><iframe src="https://example.com"></iframe></foreignObject>',
    "<!-- comment -->",
    "<![CDATA[x]]>",
    '<?xml-stylesheet href="https://example.com/s.css"?>',
    '<feImage href="https://example.com/x.png"/>',
  ]) {
    assert.equal(sanitizeSlideSvg(wrap(inner)), null, inner);
  }
  assert.equal(sanitizeSlideSvg(`<!DOCTYPE svg [<!ENTITY x SYSTEM "https://example.com/">]>${wrap("")}`), null);
  assert.equal(sanitizeSlideSvg("<html></html>"), null);
  assert.equal(sanitizeSlideSvg(wrap("").slice(0, -3)), null);
  // Event-looking text inside a quoted value (e.g. a deck font name) is not an attribute.
  const fontName = wrap('<text style="font-family:Arial onload=x">가</text>');
  assert.equal(sanitizeSlideSvg(fontName), fontName);
});
