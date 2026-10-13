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

await test("the fixture deck opens with two 960×540 slides", async () => {
  const deck = await fixtureDeck();
  assert.equal(deck.slides.length, 2);
  assert.equal(deck.width, FIXTURE_PPTX_SLIDE_W);
  assert.equal(deck.height, FIXTURE_PPTX_SLIDE_H);
});

await test("slide 1 lays out Korean/emoji text, runs, list, table, shapes and the embedded picture", async () => {
  const deck = await fixtureDeck();
  const svg = svgOf(deck, 0);
  const text = DEFAULT_PPTX_TEXT;
  assert.match(svg, /^<svg [^>]*viewBox="0 0 960\.00 540\.00"/);
  for (const part of [
    text.title,
    text.body.trim(),
    text.bold,
    text.link,
    text.scriptLink,
    ...text.list,
    ...text.table,
  ]) {
    assert.ok(svg.includes(part), part);
  }
  assert.ok(!svg.includes(text.secondSlide));
  // Title 28 pt bold; body runs 20 pt, the bold one weighted 700.
  assert.match(svg, /font-size:37\.33px[^"]*font-weight:700">FVOCI PPTX 슬라이드/);
  assert.match(svg, /font-weight:700">굵은 글씨/);
  // Two-level bullets: marker characters and a deeper indent for the second level.
  assert.match(
    svg,
    /padding-inline-start:24\.00px;text-indent:-18\.00px"><span[^>]*>•<\/span><span[^>]*>첫째 항목/,
  );
  assert.match(
    svg,
    /padding-inline-start:56\.00px;text-indent:-18\.00px"><span[^>]*>–<\/span><span[^>]*>하위 항목/,
  );
  // Table: red first cell and bordered grid; shapes: green rectangle, orange ellipse.
  assert.match(
    svg,
    /<rect x="480\.00" y="260\.00" width="160\.00" height="40\.00" fill="#FF0000"\/>/,
  );
  assert.ok((svg.match(/<line [^>]*stroke="#000000"/g) ?? []).length >= 8);
  assert.match(svg, /<rect x="40\.00" y="380\.00" width="160\.00" height="80\.00" fill="#00B050"/);
  // The ellipse (centre 320,420, radii 80×40) as four cubic arcs.
  assert.ok(
    svg.includes(
      '<path d="M240.00,420.00 C240.00,397.91 275.82,380.00 320.00,380.00 C364.18,380.00 400.00,397.91 400.00,420.00 C400.00,442.09 364.18,460.00 320.00,460.00 C275.82,460.00 240.00,442.09 240.00,420.00 Z" fill="#FFC000"',
    ),
  );
  // The embedded PNG is inlined at its 96×48 extent; the linked picture is only a labelled placeholder.
  assert.match(
    svg,
    /<image x="480\.00" y="400\.00" width="96\.00" height="48\.00" href="data:image\/png;base64,/,
  );
  assert.equal((svg.match(/<image /g) ?? []).length, 1);
  assert.match(svg, /data-pptx-fallback="image"/);
  assert.ok(!new RegExp(`(href|src)="${FIXTURE_PPTX_EXTERNAL_IMAGE}`).test(svg));
  // Its label (wider than the 48 px box, beside the blue picture) is cut to the box (pptx-fallback.ts).
  assert.ok(
    svg.includes(
      '<svg x="640" y="400" width="48" height="48" viewBox="640 400 48 48" overflow="hidden"><text x="664.00" y="424.00" ',
    ),
  );
  // Raw renderer output still links out; the viewer shows it only as an image inside an image (pptx-svg.ts).
  assert.ok(svg.includes(`href="${FIXTURE_PPTX_EXTERNAL_LINK}"`));
  assert.ok(svg.includes('href="javascript:alert(1)"'));
});

await test("slide 2 carries only its own text", async () => {
  const deck = await fixtureDeck();
  const svg = svgOf(deck, 1);
  assert.ok(svg.includes(DEFAULT_PPTX_TEXT.secondSlide));
  assert.ok(!svg.includes(DEFAULT_PPTX_TEXT.title));
  assert.equal(renderSlide(deck, 2).status, "failed");
});

await test("slide count and rendered size caps", async () => {
  assert.deepEqual(await openPptx(buildFixturePptx(), alive, { ...PPTX_LIMITS, maxSlides: 1 }), {
    status: "tooLarge",
  });
  const deck = await fixtureDeck();
  // A test bound between slide 1 (~10 kB of markup) and slide 2 (~1 kB).
  assert.deepEqual(renderSlide(deck, 0, { ...PPTX_LIMITS, maxSlideSvgBytes: 2000 }), {
    status: "tooLarge",
  });
  assert.equal(renderSlide(deck, 1, { ...PPTX_LIMITS, maxSlideSvgBytes: 2000 }).status, "ok");
  // Slide 2 is shown at exactly its own size and refused one unit under it.
  const size = svgOf(deck, 1).length;
  assert.equal(renderSlide(deck, 1, { ...PPTX_LIMITS, maxSlideSvgBytes: size }).status, "ok");
  assert.deepEqual(renderSlide(deck, 1, { ...PPTX_LIMITS, maxSlideSvgBytes: size - 1 }), {
    status: "tooLarge",
  });
});

await test("packages over the inflate cap or that are not decks are refused", async () => {
  const inflated = new Uint8Array(200 * 1024 * 1024);
  const bomb = writeZip([
    { name: "[Content_Types].xml", bytes: new TextEncoder().encode("<Types/>") },
    {
      name: "ppt/presentation.xml",
      deflated: deflateRawSync(inflated),
      crc: zlibCrc32(inflated),
      size: inflated.byteLength,
    },
  ]);
  assert.deepEqual(await openPptx(bomb, alive), { status: "tooLarge" });
  assert.deepEqual(await openPptx(new TextEncoder().encode("plain text"), alive), {
    status: "invalid",
  });
  // A ZIP without [Content_Types].xml, and one with no slides.
  assert.deepEqual(
    await openPptx(
      writeZip([{ name: "ppt/presentation.xml", bytes: new TextEncoder().encode("<p/>") }]),
      alive,
    ),
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

await test("cancellation during the package check opens nothing", async () => {
  assert.deepEqual(await openPptx(buildFixturePptx(), () => false), { status: "invalid" });
});
