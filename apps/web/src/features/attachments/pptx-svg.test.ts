import assert from "node:assert/strict";
import test from "node:test";
import { openPptx, renderSlide, renderSlideImage, type PptxDeck } from "./pptx-deck.ts";
import { buildChartPptx, HOSTILE_PPTX_MARKUP } from "./pptx-hostile-fixture.ts";
import { innerSlideSvg, slideImageSvg, toBase64 } from "./pptx-svg.ts";
import { buildFixturePptx, DEFAULT_PPTX_TEXT, FIXTURE_PPTX_SLIDE_H, FIXTURE_PPTX_SLIDE_W } from "./pptx-test-fixture.ts";

async function deckOf(bytes: Uint8Array): Promise<PptxDeck> {
  const opened = await openPptx(bytes, () => true);
  assert.equal(opened.status, "ok");
  return (opened as { deck: PptxDeck }).deck;
}

function rawSvg(deck: PptxDeck, index: number): string {
  const rendered = renderSlide(deck, index);
  assert.equal(rendered.status, "ok");
  return (rendered as { svg: string }).svg;
}

function imageSvg(deck: PptxDeck, index: number): string {
  const image = renderSlideImage(deck, index);
  assert.equal(image.status, "ok");
  return (image as { svg: string }).svg;
}

const TEMPLATE = (w: string, h: string, base64: string) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
  `<image width="${w}" height="${h}" href="data:image/svg+xml;base64,${base64}"/></svg>`;

test("the served slide is exactly the fixed template around the renderer's SVG, byte for byte", async () => {
  const deck = await deckOf(buildFixturePptx());
  for (const index of [0, 1]) {
    const raw = rawSvg(deck, index);
    const outer = imageSvg(deck, index);
    assert.equal(outer, TEMPLATE("960", "540", Buffer.from(raw, "utf8").toString("base64")));
    assert.equal(innerSlideSvg(outer), raw);
  }
  // Korean text, links (inert as an image), foreignObject text and the embedded picture are all still inside.
  const inner = innerSlideSvg(imageSvg(deck, 0))!;
  for (const part of [DEFAULT_PPTX_TEXT.title, DEFAULT_PPTX_TEXT.link, ...DEFAULT_PPTX_TEXT.table]) {
    assert.ok(inner.includes(part), part);
  }
  assert.match(inner, /<foreignObject/);
  assert.match(inner, /<image [^>]*href="data:image\/png;base64,/);
  assert.equal(deck.width, FIXTURE_PPTX_SLIDE_W);
  assert.equal(deck.height, FIXTURE_PPTX_SLIDE_H);
});

test("review B1 counterexamples: chart number-format markup reaches the renderer, never the outer document", async () => {
  for (const [name, markup] of Object.entries(HOSTILE_PPTX_MARKUP)) {
    const deck = await deckOf(await buildChartPptx(markup));
    const raw = rawSvg(deck, 0);
    // Negative control: the renderer really does emit the deck's markup unescaped.
    assert.ok(raw.includes(markup), name);
    const outer = imageSvg(deck, 0);
    const match = /^<svg [^<>]*><image [^<>]*href="data:image\/svg\+xml;base64,([A-Za-z0-9+/=]*)"\/><\/svg>$/.exec(outer);
    assert.ok(match, name);
    assert.equal(outer, TEMPLATE("1280", "720", match[1]!), name);
    assert.equal(innerSlideSvg(outer), raw, name);
  }
});

test("base64 is standard and chunk joins are exact", () => {
  let seed = 7;
  const random = (n: number) =>
    Uint8Array.from({ length: n }, () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed & 0xff;
    });
  for (const n of [0, 1, 2, 3, 4, 24_575, 24_576, 24_577, 49_152, 100_003]) {
    const bytes = random(n);
    assert.equal(toBase64(bytes), Buffer.from(bytes).toString("base64"), String(n));
  }
});

test("UTF-8: multi-byte text round-trips, a lone surrogate becomes U+FFFD, the cap counts bytes", () => {
  const inner = '<svg viewBox="0 0 1 1"><text>한글 😀 &amp;</text></svg>';
  const ok = slideImageSvg(inner, 1, 1, 1024);
  assert.equal(ok.status, "ok");
  assert.equal(innerSlideSvg((ok as { svg: string }).svg), inner);

  const lone = slideImageSvg('<svg viewBox="0 0 1 1"><text>a\uD800b</text></svg>', 1, 1, 1024);
  assert.equal(innerSlideSvg((lone as { svg: string }).svg), '<svg viewBox="0 0 1 1"><text>a�b</text></svg>');

  const bytes = new TextEncoder().encode(inner).byteLength;
  assert.ok(bytes > inner.length);
  assert.equal(slideImageSvg(inner, 1, 1, bytes).status, "ok");
  // Fits in UTF-16 code units, not in UTF-8 bytes.
  assert.deepEqual(slideImageSvg(inner, 1, 1, bytes - 1), { status: "tooLarge" });
  assert.deepEqual(slideImageSvg(inner, 1, 1, inner.length - 1), { status: "tooLarge" });
});

test("only a renderer <svg> root and sane dimensions are wrapped", () => {
  for (const inner of ["", "<html></html>", ' <svg viewBox="0 0 1 1"/>', "<svgx/>", '<?xml version="1.0"?><svg/>']) {
    assert.deepEqual(slideImageSvg(inner, 1, 1, 1024), { status: "failed" }, inner);
  }
  for (const [w, h] of [
    [0, 1],
    [1, -1],
    [Number.NaN, 1],
    [1, Number.POSITIVE_INFINITY],
    [1e7, 1],
    [0.004, 1],
    [1, 0.0049],
  ]) {
    assert.deepEqual(slideImageSvg("<svg></svg>", w!, h!, 1024), { status: "failed" }, `${w}x${h}`);
  }
  // The smallest dimension the template can write.
  const smallest = slideImageSvg("<svg></svg>", 0.005, 1e6, 1024);
  assert.ok((smallest as { svg: string }).svg.startsWith('<svg xmlns="http://www.w3.org/2000/svg" width="0.01" height="1000000" '));
  const fractional = slideImageSvg("<svg></svg>", 960.004, 540.126, 1024);
  assert.ok((fractional as { svg: string }).svg.startsWith('<svg xmlns="http://www.w3.org/2000/svg" width="960" height="540.13" '));
});

test("innerSlideSvg accepts the exact template only", () => {
  const outer = (slideImageSvg("<svg>한</svg>", 10, 20, 1024) as { svg: string }).svg;
  assert.equal(innerSlideSvg(outer), "<svg>한</svg>");
  for (const other of [
    `${outer}<script/>`,
    `<!--x-->${outer}`,
    outer.replace('width="10" height="20" viewBox', 'width="10" height="21" viewBox'),
    outer.replace("base64,", "base64,*"),
    outer.replace("<image ", '<image onload="x" '),
    TEMPLATE("10", "20", "gA=="), // 0x80: not UTF-8
    TEMPLATE("10", "20", "PHN2Zz4"), // unpadded
  ]) {
    assert.equal(innerSlideSvg(other), null, other.slice(0, 80));
  }
});
