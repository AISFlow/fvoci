/**
 * What the frame does to a slide the renderer drew before it goes into the
 * page (`pptx-view.ts`), in one walk over it:
 *
 * - every font family it names is followed by the app's own, so that Korean
 *   text falls back to Noto Sans KR rather than to whatever the browser picks
 *   (the renderer names the deck's fonts, "맑은 고딕", "Calibri", alone);
 * - the box the renderer draws where a chart was (the worker leaves charts
 *   out) says so, in the words the app gave (`Labels` of the protocol);
 * - nothing in it can load or go anywhere: links lose their addresses (a
 *   jump to another slide is a listener, not an address), a picture or a
 *   style keeps only addresses made in memory (`blob:`, `data:` pictures)
 *   or within the slide (`#…`), and players and frames are taken out.
 *
 * The worker already left out every relationship to outside the file, and
 * the frame's policy keeps it off the network: this is a defense besides.
 */
/** What every font family falls back to: the app's families, as the frame's style sheet has them. */
const FALLBACK = '"Noto Sans KR", sans-serif';
/** What the renderer writes in the box of a chart it does not have. */
const NO_CHART = 'Chart not found';
/** Marks the box of a chart, for the tests. */
export const CHART_MARK = 'data-fvoci-chart';

const PLAYERS = 'video, audio, iframe, object, embed, script, link, meta, base, form';
const ADDRESS = /url\(\s*(['"]?)(.*?)\1\s*\)/g;

function withFallback(family: string): string {
  return family === '' || family.includes('Noto Sans KR') ? family : `${family}, ${FALLBACK}`;
}

/** Whether an address stays in the frame: made in memory, or a place in the slide. */
function inFrame(address: string): boolean {
  const value = address.trim();
  return value.startsWith('#') || value.startsWith('blob:') || /^data:image\//i.test(value);
}

function cleanStyle(element: HTMLElement | SVGElement): void {
  const { style } = element;
  for (const name of [...style]) {
    const value = style.getPropertyValue(name);
    if (!value.includes('url(')) continue;
    const addresses = [...value.matchAll(ADDRESS)].map((match) => match[2] ?? '');
    if (!addresses.every(inFrame)) style.removeProperty(name);
  }
  if (style.fontFamily) style.fontFamily = withFallback(style.fontFamily);
}

function cleanAttributes(element: Element): void {
  for (const { name, value } of [...element.attributes]) {
    const lower = name.toLowerCase();
    if (lower.startsWith('on')) element.removeAttribute(name);
    else if (lower === 'href' || lower === 'xlink:href' || lower === 'src' || lower === 'srcset') {
      // A link goes nowhere but within the slide; anything else loads only what is in memory.
      const link = element.localName === 'a' || element.localName === 'area';
      if (link ? !value.trim().startsWith('#') : !inFrame(value)) element.removeAttribute(name);
    } else if (lower === 'font-family') {
      element.setAttribute(name, withFallback(value));
    }
  }
}

/** Cleans the slide in place; `chart` is what the box of a chart says. */
export function cleanSlide(slide: HTMLElement, chart: string): void {
  for (const player of slide.querySelectorAll(PLAYERS)) player.remove();
  for (const element of slide.querySelectorAll('*')) {
    cleanAttributes(element);
    if (element instanceof HTMLElement || element instanceof SVGElement) cleanStyle(element);
    // The renderer's box (a dashed border, its text centred), not text of the deck's that says the same.
    if (
      element instanceof HTMLElement &&
      element.childElementCount === 0 &&
      element.textContent === NO_CHART &&
      element.style.borderStyle === 'dashed' &&
      element.style.justifyContent === 'center'
    ) {
      element.textContent = chart;
      element.setAttribute(CHART_MARK, '');
    }
  }
}
