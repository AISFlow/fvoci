import assert from "node:assert/strict";
import test from "node:test";
import { renderSlideToSvg } from "@office-kit/pptx-preview";
import { boundFallbackLabels } from "./pptx-fallback.ts";
import { openPptx, renderSlide } from "./pptx-deck.ts";
import { buildFixturePptx, FIXTURE_PPTX_EXTERNAL_IMAGE } from "./pptx-test-fixture.ts";

const alive = () => true;
const NS = 'xmlns="http://www.w3.org/2000/svg"';
const RECT = (x: string, y: string, w: string, h: string) =>
  `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="#F3F4F6"/>`;
const LABEL =
  '<text x="24.00" y="24.00" text-anchor="middle">picture (link: https://example.com/a.png)</text>';

function bounded(svg: string): string {
  const result = boundFallbackLabels(svg);
  assert.equal(result.status, "ok", svg);
  return (result as { svg: string }).svg;
}

/** The output with the inserted viewport tags taken out: the input, byte for byte. */
function withoutViewports(svg: string): string {
  return svg.replace(
    /<svg (?:x="[^"]*" y="[^"]*" )?width="[^"]*" height="[^"]*"(?: viewBox="[^"]*" overflow="hidden")?>(<text[\s\S]*?<\/text>|<text[^>]*\/>)<\/svg>/g,
    "$1",
  );
}

test("markup without a fallback group is returned as is, unparsed", () => {
  const svg = "<svg><not well-formed";
  assert.deepEqual(boundFallbackLabels(svg), { status: "ok", svg });
});

test("a labelled fallback's text is wrapped in a viewport of exactly its rect, nothing else changes", () => {
  for (const kind of ["image", "chart", "graphicFrame"]) {
    const svg = `<svg ${NS}><g data-pptx-fallback="${kind}" transform="rotate(90 24.00 24.00)">${RECT("0.00", "0.00", "48.00", "48.00")}<title>t</title>${LABEL}<g><text>overlay</text></g></g></svg>`;
    const out = bounded(svg);
    assert.equal(
      out,
      svg.replace(
        LABEL,
        `<svg x="0" y="0" width="48" height="48" viewBox="0 0 48 48" overflow="hidden">${LABEL}</svg>`,
      ),
    );
    assert.equal(withoutViewports(out), svg);
  }
});

test("offsets hold across non-BMP text, entities, CRLF and nested groups; every fallback is bounded", () => {
  const svg =
    `<svg ${NS}>\r\n<g><text>🙂🚀 &amp; 가</text><g transform="scale(2)">` +
    `<g data-pptx-fallback="image">${RECT("-10.50", "7.25", "24.00", "12.00")}<text x="1">🙂 a&lt;b</text></g></g>` +
    `<g data-pptx-fallback="chart">${RECT("100.00", "0.00", "10.00", "10.00")}<text/></g></g></svg>`;
  const out = bounded(svg);
  assert.ok(
    out.includes(
      '<svg x="-10.5" y="7.25" width="24" height="12" viewBox="-10.5 7.25 24 12" overflow="hidden"><text x="1">🙂 a&lt;b</text></svg></g>',
    ),
  );
  assert.ok(
    out.includes(
      '<svg x="100" y="0" width="10" height="10" viewBox="100 0 10 10" overflow="hidden"><text/></svg></g>',
    ),
  );
  assert.equal(withoutViewports(out), svg);
});

test("offsets hold across CDATA sections, comments and processing instructions holding tag text", () => {
  const label = "<text><![CDATA[🙂 </text> <g> 가]]></text>";
  const svg =
    `<?xml version="1.0"?>\r\n<!-- <g data-pptx-fallback="image"> 🚀 -->\r\n<svg ${NS}><?pi <text>?>` +
    `<text><![CDATA[😀\r\n</g>]]></text><g data-pptx-fallback="image">${RECT("1.00", "2.00", "3.00", "4.00")}<!-- 🙂 -->${label}</g></svg>`;
  const out = bounded(svg);
  assert.equal(
    out,
    svg.replace(
      label,
      `<svg x="1" y="2" width="3" height="4" viewBox="1 2 3 4" overflow="hidden">${label}</svg>`,
    ),
  );
});

test("a DOCTYPE's entities are not expanded and nothing is loaded: an entity reference is failed", () => {
  const svgs = [
    `<!DOCTYPE svg [<!ENTITY e "x">]><svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "4", "4")}<text>&e;</text></g></svg>`,
    `<!DOCTYPE svg [<!ENTITY e SYSTEM "https://example.com/e">]><svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "4", "4")}<text>&e;</text></g></svg>`,
  ];
  for (const svg of svgs) assert.deepEqual(boundFallbackLabels(svg), { status: "failed" }, svg);
});

test("a box without area shows no label", () => {
  for (const [w, h] of [
    ["0.00", "48.00"],
    ["48.00", "0.00"],
    ["-4.00", "48.00"],
  ]) {
    const svg = `<svg ${NS}><g data-pptx-fallback="image">${RECT("0.00", "0.00", w!, h!)}${LABEL}</g></svg>`;
    assert.equal(bounded(svg), svg.replace(LABEL, `<svg width="0" height="0">${LABEL}</svg>`));
  }
});

test("other groups carrying the marker are left alone", () => {
  const svgs = [
    // custGeom marks real geometry, not a labelled placeholder.
    `<svg ${NS}><g data-pptx-fallback="custGeom"><path d="M0 0"/><text>x</text></g></svg>`,
    // Not in the SVG namespace, or the marker in another namespace.
    `<svg ${NS}><x:g xmlns:x="urn:x" data-pptx-fallback="image"><text>x</text></x:g></svg>`,
    `<svg ${NS} xmlns:x="urn:x"><g x:data-pptx-fallback="image"><text>x</text></g></svg>`,
    // Marker text in character data only.
    `<svg ${NS}><text>data-pptx-fallback</text></svg>`,
  ];
  for (const svg of svgs) assert.equal(bounded(svg), svg);
});

test("markup that is not well-formed, or a fallback group of another shape, is failed", () => {
  const bad = [
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "4", "4")}${LABEL}</svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "4", "4")}<text>&nbsp;</text></g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${LABEL}</g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "4", "4")}</g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "4", "4")}<g/>${LABEL}</g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "Infinity", "4")}${LABEL}</g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "", "4")}${LABEL}</g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image">${RECT("0", "0", "9".repeat(400), "4")}${LABEL}</g></svg>`,
    `<svg ${NS}><g data-pptx-fallback="image"><rect x="0" y="0" width="4"/>${LABEL}</g></svg>`,
  ];
  for (const svg of bad) assert.deepEqual(boundFallbackLabels(svg), { status: "failed" }, svg);
});

test("renderer placeholders: linked, grouped, rotated and missing pictures are cut to their own box", async () => {
  const opened = await openPptx(buildFixturePptx(undefined, { slide2Fallbacks: true }), alive);
  assert.equal(opened.status, "ok");
  const deck = (opened as { deck: Parameters<typeof renderSlide>[0] }).deck;
  const raw = renderSlideToSvg(deck.pres, deck.slides[1]!);
  const rendered = renderSlide(deck, 1);
  assert.equal(rendered.status, "ok");
  const svg = (rendered as { svg: string }).svg;
  // The SDK's own output: three labelled placeholders, none bounded.
  assert.equal((raw.match(/data-pptx-fallback="image"/g) ?? []).length, 3);
  assert.ok(!/<svg [^>]*overflow="hidden"><text /.test(raw));
  // Grouped: the viewport is in the child space the group transform scales, as the rect is.
  assert.match(
    svg,
    /<g transform="translate\(100\.00 300\.00\) scale\(2\.000000 2\.000000\)"><g data-pptx-fallback="image"><rect x="10\.00" y="10\.00" width="24\.00" height="24\.00" [^>]*\/><title>[^<]*<\/title><svg x="10" y="10" width="24" height="24" viewBox="10 10 24 24" overflow="hidden"><text x="22\.00" y="22\.00" [^>]*>picture \(link: https:\/\/example\.com\/pptx-tracker\.png\)<\/text><\/svg><\/g>/,
  );
  // Rotated: the viewport sits inside the rotated group, with the rect.
  assert.match(
    svg,
    /<g data-pptx-fallback="image" transform="rotate\(90 424\.00 324\.00\)"><rect x="400\.00" y="300\.00" [^>]*\/><title>[^<]*<\/title><svg x="400" y="300" width="48" height="48" viewBox="400 300 48 48" overflow="hidden"><text /,
  );
  // Missing embed.
  assert.match(
    svg,
    /<svg x="600" y="300" width="48" height="48" viewBox="600 300 48 48" overflow="hidden"><text [^>]*>picture \(no bytes\)<\/text><\/svg>/,
  );
  // Nothing else changed, and the linked picture is still never referenced.
  assert.equal(withoutViewports(svg), raw);
  assert.ok(!new RegExp(`(href|src)="${FIXTURE_PPTX_EXTERNAL_IMAGE}`).test(svg));
});
