import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync, crc32 as zlibCrc32 } from "node:zlib";
import { writeZip } from "./docx-test-fixture.ts";
import { openPptx, PPTX_LIMITS, renderSlide, type PptxDeck } from "./pptx-deck.ts";
import {
  buildFixturePptx,
  DEFAULT_PPTX_TEXT,
  FIXTURE_PPTX_EXTERNAL_IMAGE,
  FIXTURE_PPTX_EXTERNAL_LINK,
  FIXTURE_PPTX_SLIDE_H,
  FIXTURE_PPTX_SLIDE_W,
} from "./pptx-test-fixture.ts";

const alive = () => true;

async function fixtureDeck(): Promise<PptxDeck> {
  const opened = await openPptx(buildFixturePptx(), alive);
  assert.equal(opened.status, "ok");
  return (opened as { deck: PptxDeck }).deck;
}

function svgOf(deck: PptxDeck, index: number): string {
  const rendered = renderSlide(deck, index);
  assert.equal(rendered.status, "ok");
  return (rendered as { svg: string }).svg;
}

test("the fixture deck opens with two 960×540 slides", async () => {
  const deck = await fixtureDeck();
  assert.equal(deck.slides.length, 2);
  assert.equal(deck.width, FIXTURE_PPTX_SLIDE_W);
  assert.equal(deck.height, FIXTURE_PPTX_SLIDE_H);
});

test("slide 1 lays out Korean/emoji text, runs, list, table, shapes and the embedded picture", async () => {
  const deck = await fixtureDeck();
  const svg = svgOf(deck, 0);
  const text = DEFAULT_PPTX_TEXT;
  assert.match(svg, /^<svg [^>]*viewBox="0 0 960\.00 540\.00"/);
  for (const part of [text.title, text.body.trim(), text.bold, text.link, text.scriptLink, ...text.list, ...text.table]) {
    assert.ok(svg.includes(part), part);
  }
  assert.ok(!svg.includes(text.secondSlide));
  // Title 36 pt bold; body runs 20 pt, the bold one weighted 700.
  assert.match(svg, /font-size:48\.00px[^"]*font-weight:700">FVOCI PPTX 슬라이드/);
  assert.match(svg, /font-weight:700">굵은 글씨/);
  // Two-level bullets: marker characters and a deeper indent for the second level.
  assert.match(svg, /padding-left:24\.00px;text-indent:-18\.00px"><span[^>]*>•<\/span><span[^>]*>첫째 항목/);
  assert.match(svg, /padding-left:56\.00px;text-indent:-18\.00px"><span[^>]*>–<\/span><span[^>]*>하위 항목/);
  // Table: red first cell and bordered grid; shapes: green rectangle, orange ellipse.
  assert.match(svg, /<rect x="480\.00" y="260\.00" width="160\.00" height="40\.00" fill="#FF0000"\/>/);
  assert.ok((svg.match(/<line [^>]*stroke="#000000"/g) ?? []).length >= 8);
  assert.match(svg, /<rect x="40\.00" y="380\.00" width="160\.00" height="80\.00" fill="#00B050"/);
  assert.match(svg, /<ellipse cx="320\.00" cy="420\.00" rx="80\.00" ry="40\.00" fill="#FFC000"/);
  // The embedded PNG is inlined at its 96×48 extent; the linked picture is only a labelled placeholder.
  assert.match(svg, /<image x="480\.00" y="400\.00" width="96\.00" height="48\.00" href="data:image\/png;base64,/);
  assert.equal((svg.match(/<image /g) ?? []).length, 1);
  assert.match(svg, /data-pptx-fallback="image"/);
  assert.ok(!new RegExp(`(href|src)="${FIXTURE_PPTX_EXTERNAL_IMAGE}`).test(svg));
  // Raw renderer output still links out; the viewer strips this (pptx-svg.ts, browser test).
  assert.ok(svg.includes(`href="${FIXTURE_PPTX_EXTERNAL_LINK}"`));
  assert.ok(svg.includes('href="javascript:alert(1)"'));
});

test("slide 2 carries only its own text", async () => {
  const deck = await fixtureDeck();
  const svg = svgOf(deck, 1);
  assert.ok(svg.includes(DEFAULT_PPTX_TEXT.secondSlide));
  assert.ok(!svg.includes(DEFAULT_PPTX_TEXT.title));
  assert.equal(renderSlide(deck, 2).status, "failed");
});

test("slide count and rendered size caps", async () => {
  assert.deepEqual(await openPptx(buildFixturePptx(), alive, { ...PPTX_LIMITS, maxSlides: 1 }), { status: "tooLarge" });
  const deck = await fixtureDeck();
  assert.deepEqual(renderSlide(deck, 0, { ...PPTX_LIMITS, maxSlideSvgChars: 1000 }), { status: "tooLarge" });
  assert.equal(renderSlide(deck, 1, { ...PPTX_LIMITS, maxSlideSvgChars: 1000 }).status, "ok");
});

test("packages over the inflate cap or that are not decks are refused", async () => {
  const inflated = new Uint8Array(200 * 1024 * 1024);
  const bomb = writeZip([
    { name: "[Content_Types].xml", bytes: new TextEncoder().encode("<Types/>") },
    { name: "ppt/presentation.xml", deflated: deflateRawSync(inflated), crc: zlibCrc32(inflated), size: inflated.byteLength },
  ]);
  assert.deepEqual(await openPptx(bomb, alive), { status: "tooLarge" });
  assert.deepEqual(await openPptx(new TextEncoder().encode("plain text"), alive), { status: "invalid" });
  // A ZIP without [Content_Types].xml, and one with no slides.
  assert.deepEqual(
    await openPptx(writeZip([{ name: "ppt/presentation.xml", bytes: new TextEncoder().encode("<p/>") }]), alive),
    { status: "invalid" },
  );
  const docx = writeZip([
    {
      name: "[Content_Types].xml",
      bytes: new TextEncoder().encode(
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>',
      ),
    },
    { name: "word/document.xml", bytes: new TextEncoder().encode("<w:document/>") },
  ]);
  assert.deepEqual(await openPptx(docx, alive), { status: "invalid" });
});

test("cancellation during the package check opens nothing", async () => {
  assert.deepEqual(await openPptx(buildFixturePptx(), () => false), { status: "invalid" });
});
