/**
 * Hostile PPTX for viewer tests (unit and browser); never bundled.
 *
 * `@office-kit/pptx-preview` 0.11 copied the quoted prefix of a chart
 * data-label number format into its SVG unescaped, so a deck could put
 * arbitrary markup into the renderer's output; 0.15 writes it as escaped text
 * (`rendererText`). The payloads still travel from the deck to the renderer's
 * output. Markup the renderer does emit as is (a `javascript:` hyperlink, as
 * in the fixture deck's script link) checks the image boundary instead. These
 * decks are built with `@office-kit/pptx`'s own writer: one column chart
 * whose data labels use `numberFormat`.
 */
import {
  addSlide,
  addSlideChart,
  createPresentation,
  emu,
  getSlideLayouts,
  savePresentation,
} from "@office-kit/pptx";

/** Payloads that passed the old regex sanitizer (review B1), plus plain ones. */
export const HOSTILE_PPTX_MARKUP = {
  prefixedScript: `<x:script xmlns:x='http://www.w3.org/2000/svg'>window.__pptxPwned=1;fetch('/pwned-script')</x:script>`,
  singleQuoteLink: `<x:a xmlns:x='http://www.w3.org/2000/svg' href='javascript:window.__pptxPwned=2'><x:text y='20'>CLICK</x:text></x:a>`,
  xhtmlIframe: `<h:iframe xmlns:h='http://www.w3.org/1999/xhtml' src='/pwned-iframe'/>`,
  xhtmlRefresh: `<h:meta xmlns:h='http://www.w3.org/1999/xhtml' http-equiv='refresh' content='0;url=/pwned-refresh'/>`,
  remoteImage: `<x:image xmlns:x='http://www.w3.org/2000/svg' href='/pwned-image.png' width='10' height='10'/>`,
  plainScript: `<script>window.__pptxPwned=3</script>`,
} as const;

/** `text` as the renderer writes text content (pptx-preview `escapeXml`). */
export function rendererText(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}

/** A one-slide deck whose chart data labels carry `markup` as a quoted number-format prefix. */
export async function buildChartPptx(markup: string): Promise<Uint8Array> {
  const pres = createPresentation();
  const layout = getSlideLayouts(pres)[0];
  if (!layout) throw new Error("fixture presentation has no slide layout");
  const slide = addSlide(pres, { layout });
  addSlideChart(slide, {
    x: emu(0),
    y: emu(0),
    w: emu(5_000_000),
    h: emu(3_000_000),
    spec: {
      kind: "column",
      categories: ["a", "b"],
      series: [{ name: "s", values: [1, 2] }],
      dataLabels: {
        showValue: true,
        showCategory: false,
        showSeriesName: false,
        showPercent: false,
        numberFormat: `"${markup}"0`,
      },
    },
  });
  return savePresentation(pres);
}
