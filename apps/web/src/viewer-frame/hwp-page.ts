/**
 * A page of an HWP document as the frame shows it: the picture the worker
 * drew (SVG), kept to what a page is drawn with, and the page's text over it,
 * transparent, for selecting and for a screen reader.
 *
 * The SVG is rhwp's output for a document someone uploaded, so it is not
 * taken as it is. It is parsed as XML and copied element by element: only
 * the elements and attributes a page is drawn with (those rhwp 0.9.0 writes)
 * are copied, links give way to what they hold, a reference goes only to
 * something on the same page, and a picture only to bytes the page carries
 * (`data:image/…`). The frame's policy would stop scripts and the network as
 * well; this keeps the page from carrying them at all.
 *
 * rhwp draws each character as a `<text>` of its own, which a screen reader
 * would read a letter at a time. So the picture is hidden from it, and the
 * page's runs of text (`getPageTextLayout`) are laid over the picture where
 * they are drawn, in transparent letters stretched to the run's width.
 *
 * The ids of a page are given the page's number, as every page is in the one
 * document and rhwp names the clips of each page alike.
 */
import type { TextRun } from './hwp-messages.ts';

const SVG = 'http://www.w3.org/2000/svg';
const XLINK = 'http://www.w3.org/1999/xlink';

/** What every family a page names falls back to, ahead of the generic one: the app's Korean family. */
const FALLBACK = "'Noto Sans KR'";
const GENERIC = /^(?:serif|sans-serif|monospace|cursive|fantasy|system-ui)$/i;

const ELEMENTS = new Set([
  'svg',
  'g',
  'defs',
  'clipPath',
  'rect',
  'line',
  'path',
  'polyline',
  'polygon',
  'circle',
  'ellipse',
  'text',
  'tspan',
  'image',
  'pattern',
  'marker',
  'linearGradient',
  'radialGradient',
  'stop',
  'filter',
  'feColorMatrix',
  'feComponentTransfer',
  'feFuncR',
  'feFuncG',
  'feFuncB',
  'feFuncA',
]);

/** Elements that go but leave what they hold: a link of the document. */
const UNWRAPPED = new Set(['a']);

const ATTRIBUTES = new Set([
  ...['x', 'y', 'x1', 'y1', 'x2', 'y2', 'cx', 'cy', 'r', 'rx', 'ry', 'fx', 'fy', 'fr'],
  ...['width', 'height', 'd', 'points', 'transform', 'viewBox', 'preserveAspectRatio'],
  ...['fill', 'fill-opacity', 'fill-rule', 'opacity', 'visibility', 'display', 'overflow'],
  ...['stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray', 'stroke-dashoffset'],
  ...['stroke-linecap', 'stroke-linejoin', 'stroke-miterlimit', 'paint-order', 'vector-effect'],
  ...['clip-path', 'clip-rule', 'clipPathUnits', 'filter', 'filterUnits', 'primitiveUnits'],
  ...['font-family', 'font-size', 'font-weight', 'font-style', 'text-anchor', 'text-decoration'],
  ...['dominant-baseline', 'baseline-shift', 'letter-spacing', 'word-spacing', 'writing-mode'],
  ...['textLength', 'lengthAdjust', 'dx', 'dy', 'rotate', 'xml:space'],
  ...['id', 'offset', 'stop-color', 'stop-opacity', 'gradientUnits', 'gradientTransform'],
  ...['spreadMethod', 'patternUnits', 'patternContentUnits', 'patternTransform'],
  ...['markerWidth', 'markerHeight', 'markerUnits', 'refX', 'refY', 'orient'],
  ...['marker-start', 'marker-mid', 'marker-end', 'color', 'color-interpolation-filters'],
  ...['type', 'values', 'tableValues', 'slope', 'intercept', 'amplitude', 'exponent', 'in'],
  ...['image-rendering', 'shape-rendering', 'text-rendering', 'transform-origin'],
]);

/** Where a picture's `href` may point: bytes the page carries. */
const PICTURE = /^data:image\/[a-z0-9.+-]+[;,]/i;

/** A family list with the app's family before its generic family (or at its end). */
export function withFallback(families: string): string {
  const names = families.split(',').map((name) => name.trim());
  if (names.some((name) => name.replace(/^['"]|['"]$/g, '') === 'Noto Sans KR')) return families;
  const generic = names.findIndex((name) => GENERIC.test(name));
  names.splice(generic === -1 ? names.length : generic, 0, FALLBACK);
  return names.join(',');
}

/** An attribute's value with the page's ids named as this page names them. */
function withIds(value: string, prefix: string): string {
  return value.replace(/url\(\s*#([^)\s]+)\s*\)/g, (_, id: string) => `url(#${prefix}${id})`);
}

/** The value an attribute is copied with, or undefined when it is not copied. */
function copiedValue(element: Element, attribute: Attr, prefix: string): string | undefined {
  const name = attribute.namespaceURI === XLINK ? 'href' : attribute.name;
  const value = attribute.value;
  if (name === 'href') {
    if (value.startsWith('#')) return `#${prefix}${value.slice(1)}`;
    return element.localName === 'image' && PICTURE.test(value) ? value : undefined;
  }
  if (!ATTRIBUTES.has(name)) return undefined;
  if (name === 'id') return `${prefix}${value}`;
  if (name === 'font-family') return withFallback(value);
  // A reference (a clip, a gradient) only within the page; nothing in a value reaches further.
  // A backslash goes too: CSS reads `u\72l(` as `url(`, and no value rhwp writes has one.
  if (value.includes('\\') || /url\(\s*[^#\s]/i.test(value)) return undefined;
  return withIds(value, prefix);
}

function copy(from: Element, into: Element, prefix: string): void {
  for (const child of from.childNodes) {
    if (child.nodeType === Node.TEXT_NODE) {
      into.append(into.ownerDocument.createTextNode(child.textContent ?? ''));
      continue;
    }
    if (!(child instanceof Element) || child.namespaceURI !== SVG) continue;
    if (child.localName.toLowerCase() === 'meta') continue;
    if (UNWRAPPED.has(child.localName)) {
      copy(child, into, prefix);
      continue;
    }
    if (!ELEMENTS.has(child.localName)) continue;
    const element = into.ownerDocument.createElementNS(SVG, child.localName);
    for (const attribute of child.attributes) {
      const value = copiedValue(child, attribute, prefix);
      if (value === undefined) continue;
      if (attribute.namespaceURI === XLINK) element.setAttribute('href', value);
      else element.setAttribute(attribute.name, value);
    }
    copy(child, element, prefix);
    into.append(element);
  }
}

/**
 * The page's picture, made in `document` of what may be copied of `svg`;
 * undefined when `svg` is not an SVG document.
 */
export function pagePicture(
  svg: string,
  page: number,
  document: Document,
): SVGSVGElement | undefined {
  const parsed = new DOMParser().parseFromString(svg, 'image/svg+xml');
  const root = parsed.documentElement;
  if (root.namespaceURI !== SVG || root.localName !== 'svg') return undefined;
  if (parsed.getElementsByTagName('parsererror').length > 0) return undefined;
  const prefix = `p${page}-`;
  const picture = document.createElementNS(SVG, 'svg');
  const viewBox = root.getAttribute('viewBox');
  if (viewBox && /^[\d.\s,eE+-]+$/.test(viewBox)) picture.setAttribute('viewBox', viewBox);
  picture.setAttribute('preserveAspectRatio', 'none');
  picture.setAttribute('aria-hidden', 'true');
  copy(root, picture, prefix);
  return picture;
}

let measuring: CanvasRenderingContext2D | null | undefined;

/** How wide `text` is in the text layer's font at `size`, or undefined when it cannot be told. */
function measure(text: string, size: number): number | undefined {
  measuring ??= document.createElement('canvas').getContext('2d');
  if (!measuring) return undefined;
  measuring.font = `${size}px 'Noto Sans KR', sans-serif`;
  return measuring.measureText(text).width || undefined;
}

/** A gap between two runs of a line, past which a space parts them, in ems. */
const GAP = 0.2;

/**
 * The runs that hold text, each followed by a space where a gap parts it from
 * the next on its line (the cells of a table), the run stretched over the
 * gap: copied text keeps its words apart.
 */
function spaced(runs: readonly TextRun[]): TextRun[] {
  const kept = runs.filter((run) => run.text.trim() !== '');
  return kept.map((run, index) => {
    const next = kept[index + 1];
    if (next?.y !== run.y || /\s$/.test(run.text)) return run;
    const gap = next.x - (run.x + run.width);
    return gap > run.size * GAP ? { ...run, text: `${run.text} `, width: next.x - run.x } : run;
  });
}

/**
 * The page's text, laid out at the page's own size: a run per line, where
 * the picture draws it, each stretched to the width the picture gives it.
 * The runs follow one another as the document has them, a line break
 * between two lines, a space where a gap parts two runs of a line.
 */
export function pageText(runs: readonly TextRun[], document: Document): HTMLElement {
  const layer = document.createElement('div');
  layer.className = 'hwp-text';
  let line: number | undefined;
  for (const run of spaced(runs)) {
    if (line !== undefined) layer.append(document.createElement(run.y === line ? 'wbr' : 'br'));
    line = run.y;
    const span = document.createElement('span');
    span.textContent = run.text;
    span.style.left = `${run.x}px`;
    span.style.top = `${run.y}px`;
    span.style.fontSize = `${run.size}px`;
    const natural = measure(run.text, run.size);
    if (natural) span.style.transform = `scaleX(${run.width / natural})`;
    layer.append(span);
  }
  return layer;
}
