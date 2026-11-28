/**
 * The icons of a toggle and of a callout, drawn alike where a body is
 * edited and where it is only shown: the outlines of Lucide's (ISC), in the
 * colour of the text around them, hidden from assistive technology (what
 * they say is said in words next to them).
 */
import type { CalloutTone } from './schema-rules.ts';

const SVG = 'http://www.w3.org/2000/svg';

/** The chevron of a toggle, pointing right; turned down while the toggle is open (editor.css). */
const CHEVRON = ['m9 18 6-6-6-6'];

/** The icon of each tone of a callout: a note, a light bulb, a warning sign, a stop sign. */
const TONE_ICONS: Record<CalloutTone, string[]> = {
  info: ['M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0', 'M12 16v-4', 'M12 8h.01'],
  tip: [
    'M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5',
    'M9 18h6',
    'M10 22h4',
  ],
  warn: [
    'm21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3',
    'M12 9v4',
    'M12 17h.01',
  ],
  danger: [
    'M15.312 2a2 2 0 0 1 1.414.586l4.688 4.688A2 2 0 0 1 22 8.688v6.624a2 2 0 0 1-.586 1.414l-4.688 4.688a2 2 0 0 1-1.414.586H8.688a2 2 0 0 1-1.414-.586l-4.688-4.688A2 2 0 0 1 2 15.312V8.688a2 2 0 0 1 .586-1.414l4.688-4.688A2 2 0 0 1 8.688 2z',
    'M12 8v4',
    'M12 16h.01',
  ],
};

function outline(page: Document, paths: readonly string[], name: string): SVGSVGElement {
  const svg = page.createElementNS(SVG, 'svg');
  const attributes = {
    class: name,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': '2',
    'stroke-linecap': 'round',
    'stroke-linejoin': 'round',
    'aria-hidden': 'true',
    focusable: 'false',
  };
  for (const [key, value] of Object.entries(attributes)) svg.setAttribute(key, value);
  for (const shape of paths) {
    const part = page.createElementNS(SVG, 'path');
    part.setAttribute('d', shape);
    svg.append(part);
  }
  return svg;
}

/** The chevron of a toggle. */
export const chevronIcon = (page: Document): SVGSVGElement =>
  outline(page, CHEVRON, 'doc-details-chevron');

/** The icon of a callout's tone. */
export const toneIcon = (page: Document, tone: CalloutTone): SVGSVGElement =>
  outline(page, TONE_ICONS[tone], 'doc-callout-glyph');
