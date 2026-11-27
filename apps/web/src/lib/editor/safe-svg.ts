/**
 * Turns the markup of a drawn diagram into a picture that is safe to show.
 *
 * The diagram library takes text that one user wrote and gives back an SVG
 * picture as a string, which is then shown to other users. The library
 * cleans its own output, but what it lets through is decided there and may
 * change with a version. So the string is not trusted: it is read into a
 * document of its own, where nothing runs and nothing is fetched, and the
 * picture that goes into the page is built anew, element by element, from
 * the lists in this file. What is not on a list is not copied.
 *
 * What cannot get through, whatever the library does:
 *
 * - scripts, and any attribute that holds a script (`on…`);
 * - links (`<a>` is dropped; what it holds is kept);
 * - anything that names an address outside the picture: `<image>`,
 *   `<feImage>`, `href` and `xlink:href` other than `#name` defined in the
 *   picture itself, `url(…)` other than `url(#name)` defined in the picture
 *   itself, `@import`, `@font-face`;
 * - `<foreignObject>` and with it every HTML element;
 * - style rules, animations, IDs, or utility classes that reach outside the
 *   picture (every selector starts with `#rootId` without sibling combinators,
 *   every inner `id` carries `rootId` as prefix or suffix, `@keyframes` and
 *   inline/sheet `animation` names are prefixed with `rootId-`, and layout
 *   escape properties are excluded while `.doc-diagram` enforces CSS containment).
 *
 * THIS IS THE ONE FILE of the read-only view that turns a string into
 * elements (the parser below). tests/editor/render-imports.test.ts names it
 * as the exception and keeps every other file free of such calls.
 */

const SVG_NAMESPACE = 'http://www.w3.org/2000/svg';

/** What a picture may be made of. Everything else (`feImage`, `foreignObject`, `script`) is dropped. */
const ELEMENTS = new Set(
  [
    // Structure and reusable definitions
    'svg g defs symbol marker clipPath mask pattern linearGradient radialGradient stop',
    // Shapes, text, accessibility labels, and local `<use>` / `<style>`
    'path rect circle ellipse line polyline polygon text tspan textPath title desc switch use style',
    // Filters operating strictly on the picture itself (`feImage` fetches and is excluded)
    'filter feDropShadow feGaussianBlur feOffset feMerge feMergeNode feFlood feComposite feColorMatrix feBlend',
  ]
    .join(' ')
    .split(' '),
);

/** Parser fallback map for newer mixed-case SVG elements (`fedropshadow` -> `feDropShadow`). */
const ELEMENT_NAMES = new Map(Array.from(ELEMENTS, (name) => [name.toLowerCase(), name]));
/** Dropped, keeping its children in place: a link around a label. */
const UNWRAPPED = new Set(['a']);
/** Elements permitted to reference another element inside the same picture via `#name`. */
const REFERENCING = new Set(['use', 'textPath', 'linearGradient', 'radialGradient', 'pattern']);

const ATTRIBUTES = new Set(
  [
    'class id role style xml:space data-look viewBox preserveAspectRatio width height',
    // Geometry
    'x y x1 y1 x2 y2 cx cy r rx ry dx dy d points transform transform-origin pathLength rotate',
    'textLength lengthAdjust startOffset markerWidth markerHeight markerUnits refX refY orient',
    // Gradients, patterns, clips, masks, and filter primitives
    'gradientUnits gradientTransform spreadMethod fx fy offset patternUnits patternContentUnits patternTransform',
    'clipPathUnits maskUnits maskContentUnits filterUnits primitiveUnits stdDeviation flood-color flood-opacity',
    'in in2 result mode operator k1 k2 k3 k4 type values fill fill-opacity fill-rule',
    // Stroke, visibility, and typography
    'stroke stroke-width stroke-opacity stroke-dasharray stroke-dashoffset stroke-linecap stroke-linejoin stroke-miterlimit',
    'opacity color stop-color stop-opacity clip-path clip-rule mask filter marker-start marker-mid marker-end',
    'overflow visibility display vector-effect shape-rendering paint-order',
    'font-family font-size font-style font-weight font-variant text-anchor text-decoration',
    'dominant-baseline alignment-baseline baseline-shift letter-spacing word-spacing writing-mode',
  ]
    .join(' ')
    .split(' '),
);

/** Safe non-IDREF ARIA attributes and local-only IDREF ARIA attributes. */
const ARIA_TEXT_ATTRIBUTES = new Set([
  'aria-label',
  'aria-roledescription',
  'aria-description',
  'aria-hidden',
]);
const ARIA_IDREF_ATTRIBUTES = new Set(['aria-labelledby', 'aria-describedby']);
const CSS_FUNCTIONS = new Set(
  [
    'rgb rgba hsl hsla var calc min max clamp translate translatex translatey',
    'rotate scale scalex scaley skewx skewy matrix drop-shadow brightness cubic-bezier steps url',
  ]
    .join(' ')
    .split(' '),
);

/**
 * Presentation, typography, sizing, and animation CSS properties a diagram may set.
 * Layout/overlay properties (`position`, `inset`, `top`, `left`, `z-index`,
 * `pointer-events`, `cursor`, `content`, `background*`, `mask-image`) are excluded.
 */
const CSS_PROPERTIES = new Set(
  [
    'alignment-baseline animation animation-delay animation-direction animation-duration',
    'animation-fill-mode animation-iteration-count animation-name animation-play-state animation-timing-function',
    'baseline-shift clip-path clip-rule color display dominant-baseline fill fill-opacity fill-rule',
    'filter flood-color flood-opacity font font-family font-size font-style font-variant font-weight',
    'height letter-spacing lighting-color line-height marker marker-end marker-mid marker-start mask',
    'max-height max-width min-height min-width opacity overflow paint-order shape-rendering',
    'stop-color stop-opacity stroke stroke-dasharray stroke-dashoffset stroke-linecap stroke-linejoin',
    'stroke-miterlimit stroke-opacity stroke-width text-anchor text-decoration',
    'transform transform-box transform-origin transition vector-effect visibility white-space width word-spacing writing-mode',
  ]
    .join(' ')
    .split(' '),
);
const ANIMATION_KEYWORDS = new Set(
  [
    'alternate alternate-reverse auto backwards both ease ease-in ease-in-out ease-out forwards',
    'infinite inherit initial linear none normal paused reverse revert running step-end step-start unset',
  ]
    .join(' ')
    .split(' '),
);

const NAME = /^[\p{L}\p{N}_][\p{L}\p{N}_.:-]{0,199}$/u;
const CLASS_TOKENS = /^[A-Za-z0-9_\s-]+$/;
const LOCAL_REFERENCE = /^#[\p{L}\p{N}_][\p{L}\p{N}_.:-]{0,199}$/u;
const LOCAL_URL = /url\(\s*(["']?)#[\p{L}\p{N}_][\p{L}\p{N}_.:-]{0,199}\1\s*\)/giu;
const URL_TARGET = /url\(\s*["']?#([\p{L}\p{N}_][\p{L}\p{N}_.:-]{0,199})["']?\s*\)/giu;
const [MAX_DEPTH, MAX_ELEMENTS, MAX_ATTRIBUTE_LENGTH, MAX_ARIA_LENGTH] = [
  64, 50_000, 200_000, 1000,
];

export interface SafeSvgOptions {
  document?: Document | undefined;
  expectedId?: string | undefined;
}

export function isSafeCssValue(value: string): boolean {
  if (value.length > MAX_ATTRIBUTE_LENGTH) return false;
  if (/[\\<>@{}]/.test(value) || value.includes('/*')) return false;
  if (/javascript\s*:|expression/i.test(value)) return false;
  for (let at = value.indexOf('('); at !== -1; at = value.indexOf('(', at + 1)) {
    const name = /([\w-]+)\s*$/.exec(value.slice(0, at))?.[1];
    if (name === undefined || !CSS_FUNCTIONS.has(name.toLowerCase())) return false;
  }
  return !/url\s*\(/i.test(value.replace(LOCAL_URL, ''));
}

function hasUncopiedUrlTarget(value: string, copiedIds: ReadonlySet<string>): boolean {
  for (const match of value.matchAll(URL_TARGET)) {
    if (!match[1] || !copiedIds.has(match[1])) return true;
  }
  return false;
}

function isSafeAriaText(value: string): boolean {
  if (value.length > MAX_ARIA_LENGTH) return false;
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i);
    if (code < 32 || code === 60 || code === 62) return false;
  }
  return true;
}

/** Splits at a separator that is not inside quotes, brackets or parentheses. */
function splitTopLevel(text: string, separator: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let quote = '';
  let start = 0;
  for (let at = 0; at < text.length; at += 1) {
    const character = text.charAt(at);
    if (quote !== '') {
      if (character === quote) quote = '';
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === '(' || character === '[') {
      depth += 1;
    } else if (character === ')' || character === ']') {
      depth -= 1;
    } else if (character === separator && depth === 0) {
      parts.push(text.slice(start, at));
      start = at + 1;
    }
  }
  parts.push(text.slice(start));
  return parts;
}

/** The declarations of a block that can be used, as `[property, value]`. */
export function safeDeclarations(
  block: string,
  allowedIds?: ReadonlySet<string>,
): [string, string][] {
  const kept: [string, string][] = [];
  for (const declaration of splitTopLevel(block, ';')) {
    const colon = declaration.indexOf(':');
    if (colon === -1) continue;
    const property = declaration.slice(0, colon).trim();
    const value = declaration.slice(colon + 1).trim();
    if (!/^-{0,2}[A-Za-z][\w-]*$/.test(property) || value === '') continue;
    const allowed = property.startsWith('--') || CSS_PROPERTIES.has(property.toLowerCase());
    if (
      allowed &&
      isSafeCssValue(value) &&
      (!allowedIds || !hasUncopiedUrlTarget(value, allowedIds))
    ) {
      kept.push([property, value]);
    }
  }
  return kept;
}

function scopeAnimationValue(property: string, value: string, prefix: string): string {
  const lower = property.toLowerCase();
  if (lower !== 'animation' && lower !== 'animation-name') return value;
  return splitTopLevel(value, ',')
    .map((part) =>
      part.replace(/(^|\s)([A-Za-z_][\w-]*)(?=$|\s|!)/g, (whole, lead: string, token: string) => {
        if (ANIMATION_KEYWORDS.has(token.toLowerCase()) || token.startsWith(`${prefix}-`)) {
          return whole;
        }
        return `${lead}${prefix}-${token}`;
      }),
    )
    .join(',');
}

const serialise = (declarations: [string, string][], prefix?: string): string =>
  declarations
    .map(([property, value]) => {
      const scoped = prefix ? scopeAnimationValue(property, value, prefix) : value;
      return `${property}:${scoped}`;
    })
    .join(';');

/**
 * Whether a selector stays strictly inside `scope`: no braces/at-rules/semicolons,
 * and no sibling combinators (`+`, `~`) that could reach outside the `<svg>`.
 */
function isScoped(selector: string, scope: string): boolean {
  if (!selector.startsWith(scope)) return false;
  let depth = 0;
  let quote = '';
  for (let i = scope.length; i < selector.length; i += 1) {
    const ch = selector.charAt(i);
    if (quote !== '') {
      if (ch === quote) quote = '';
    } else if (ch === '"' || ch === "'") quote = ch;
    else if (ch === '(' || ch === '[') depth += 1;
    else if (ch === ')' || ch === ']') {
      depth -= 1;
      if (depth < 0) return false;
    } else if (depth === 0 && (ch === '+' || ch === '~')) {
      return false;
    }
    if (ch === '{' || ch === '}' || ch === ';' || ch === '@') return false;
  }
  if (depth !== 0 || quote !== '') return false;
  const next = selector.charAt(scope.length);
  return next === '' || /[\s.>:[[]/.test(next);
}

function scopedKeyframes(
  prelude: string,
  body: string,
  prefix: string,
  allowedIds?: ReadonlySet<string>,
): string | undefined {
  const keyframes = /^@(?:-webkit-)?keyframes\s+([A-Za-z_][\w-]*)$/.exec(prelude);
  if (!keyframes?.[1]) return undefined;
  const rawName = keyframes[1];
  const scopedName = rawName.startsWith(`${prefix}-`) ? rawName : `${prefix}-${rawName}`;
  const steps: string[] = [];
  let inner = 0;
  while (inner < body.length) {
    const stepOpen = body.indexOf('{', inner);
    const stepClose = body.indexOf('}', stepOpen);
    if (stepOpen === -1 || stepClose === -1) break;
    const step = body.slice(inner, stepOpen).trim();
    const declarations = safeDeclarations(body.slice(stepOpen + 1, stepClose), allowedIds);
    if (/^(from|to|\d{1,3}(\.\d+)?%)(\s*,\s*(from|to|\d{1,3}(\.\d+)?%))*$/.test(step)) {
      steps.push(`${step}{${serialise(declarations, prefix)}}`);
    }
    inner = stepClose + 1;
  }
  return `@keyframes ${scopedName}{${steps.join('')}}`;
}

function blockAt(text: string, open: number): { body: string; end: number } | undefined {
  let depth = 0;
  let quote = '';
  for (let index = open; index < text.length; index += 1) {
    const character = text.charAt(index);
    if (quote !== '') {
      if (character === quote) quote = '';
    } else if (character === '"' || character === "'") quote = character;
    else if (character === '{') depth += 1;
    else if (character === '}') {
      depth -= 1;
      if (depth === 0) return { body: text.slice(open + 1, index), end: index + 1 };
    }
  }
  return undefined;
}

export function safeStyleSheet(
  css: string,
  scope: string,
  allowedIds?: ReadonlySet<string>,
): string {
  if (css.includes('\\') || css.includes('<') || !scope.startsWith('#')) return '';
  const prefix = scope.slice(1);
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const rules: string[] = [];
  let at = 0;
  while (at < text.length) {
    const open = text.indexOf('{', at);
    const semicolon = text.indexOf(';', at);
    if (open === -1) break;
    if (semicolon !== -1 && semicolon < open) {
      at = semicolon + 1;
      continue;
    }
    const prelude = text.slice(at, open).trim();
    const block = blockAt(text, open);
    if (!block) break;
    at = block.end;
    if (prelude.startsWith('@')) {
      const rule = scopedKeyframes(prelude, block.body, prefix, allowedIds);
      if (rule) rules.push(rule);
      continue;
    }
    if (block.body.includes('{') || block.body.includes('@')) continue;
    const selectors = splitTopLevel(prelude, ',').map((selector) => selector.trim());
    if (selectors.some((selector) => !isScoped(selector, scope))) continue;
    const declarations = safeDeclarations(block.body, allowedIds);
    if (declarations.length > 0) {
      rules.push(`${selectors.join(',')}{${serialise(declarations, prefix)}}`);
    }
  }
  return rules.join('\n');
}

/** Inner IDs carry `rootId` as prefix (`${rootId}-…`) or suffix (`…-${rootId}` for Mermaid markers). */
function isInnerScopedId(id: string, rootId: string): boolean {
  const ok = id.length > rootId.length + 1 && NAME.test(id);
  const pre = id.startsWith(`${rootId}-`) || id.startsWith(`${rootId}_`);
  const suf = id.endsWith(`-${rootId}`) || id.endsWith(`_${rootId}`);
  return ok && (pre || suf);
}

function applyInlineStyle(
  target: SVGElement,
  rawStyle: string,
  rootId: string,
  allowedIds?: ReadonlySet<string>,
): void {
  target.removeAttribute('style');
  for (const [property, declared] of safeDeclarations(rawStyle, allowedIds)) {
    const important = /\s*!important$/i.test(declared);
    const clean = scopeAnimationValue(property, declared.replace(/\s*!important$/i, ''), rootId);
    target.style.setProperty(property, clean, important ? 'important' : '');
  }
}

function copySvgAttributes(
  source: Element,
  target: Element,
  rootId: string,
  isRoot: boolean,
  copiedIds: Set<string>,
): void {
  for (const { name, value } of Array.from(source.attributes)) {
    if (value.length > MAX_ATTRIBUTE_LENGTH) continue;
    if (name === 'href' || name === 'xlink:href') {
      if (REFERENCING.has(source.localName) && LOCAL_REFERENCE.test(value)) {
        target.setAttribute('href', value);
      }
    } else if (ARIA_TEXT_ATTRIBUTES.has(name)) {
      if (isSafeAriaText(value)) target.setAttribute(name, value);
    } else if (ARIA_IDREF_ATTRIBUTES.has(name)) {
      if (value.length <= MAX_ARIA_LENGTH) target.setAttribute(name, value);
    } else if (!ATTRIBUTES.has(name)) continue;
    else if (name === 'id') {
      if (isRoot ? value === rootId : isInnerScopedId(value, rootId)) {
        target.setAttribute('id', value);
        copiedIds.add(value);
      }
    } else if (name === 'class') {
      if (CLASS_TOKENS.test(value)) target.setAttribute('class', value);
    } else if (name === 'style') applyInlineStyle(target as SVGElement, value, rootId);
    else if (isSafeCssValue(value)) target.setAttribute(name, value);
  }
}

function cleanDanglingReferences(
  svgElement: SVGSVGElement,
  rootId: string,
  scope: string,
  copiedIds: ReadonlySet<string>,
): void {
  for (const referencing of Array.from(svgElement.querySelectorAll('[href]'))) {
    const href = referencing.getAttribute('href') ?? '';
    if (!href.startsWith('#') || !copiedIds.has(href.slice(1))) {
      if (referencing.localName === 'use') referencing.remove();
      else referencing.removeAttribute('href');
    }
  }
  for (const node of [svgElement, ...Array.from(svgElement.querySelectorAll('*'))]) {
    if (node.localName === 'style') {
      const filtered = safeStyleSheet(node.textContent, scope, copiedIds);
      if (filtered === '') node.remove();
      else node.textContent = filtered;
      continue;
    }
    for (const { name, value } of Array.from(node.attributes)) {
      if (ARIA_IDREF_ATTRIBUTES.has(name)) {
        const kept = value.split(/\s+/).filter((id) => id !== '' && copiedIds.has(id));
        if (kept.length === 0) node.removeAttribute(name);
        else node.setAttribute(name, kept.join(' '));
      } else if (name === 'style') {
        if (hasUncopiedUrlTarget(value, copiedIds)) {
          applyInlineStyle(node as SVGElement, value, rootId, copiedIds);
        }
      } else if (name !== 'id' && name !== 'href' && hasUncopiedUrlTarget(value, copiedIds)) {
        node.removeAttribute(name);
      }
    }
  }
}

/**
 * The picture, rebuilt from what is allowed; undefined when the markup is
 * not one SVG picture, or is too large or too deep to be one.
 */
export function safeSvg(markup: string, options: SafeSvgOptions = {}): SVGSVGElement | undefined {
  const page = options.document ?? document;
  const parsed = new DOMParser().parseFromString(markup, 'text/html');
  const root = parsed.body.firstElementChild;
  if (parsed.body.children.length !== 1 || !root) return undefined;
  if (root.localName !== 'svg' || root.namespaceURI !== SVG_NAMESPACE) return undefined;
  if (root.childElementCount >= MAX_ELEMENTS) return undefined;
  const rootId = root.getAttribute('id') ?? '';
  if (!NAME.test(rootId)) return undefined;
  if (options.expectedId !== undefined && rootId !== options.expectedId) return undefined;
  const scope = `#${rootId}`;

  const seen = { count: 0, tooLarge: false };
  const copiedIds = new Set<string>([rootId]);

  function copy(source: Element, depth: number, isRoot = false): Node | undefined {
    if (seen.tooLarge) return undefined;
    seen.count += 1;
    if (seen.count > MAX_ELEMENTS || depth > MAX_DEPTH) {
      seen.tooLarge = true;
      return undefined;
    }
    if (source.namespaceURI !== SVG_NAMESPACE) return undefined;
    if (UNWRAPPED.has(source.localName)) {
      const kept = page.createDocumentFragment();
      copyChildren(source, kept, depth);
      return kept;
    }
    const name = ELEMENT_NAMES.get(source.localName.toLowerCase());
    if (name === undefined) return undefined;

    const target = page.createElementNS(SVG_NAMESPACE, name);
    copySvgAttributes(source, target, rootId, isRoot, copiedIds);
    if (name === 'style') {
      const rules = safeStyleSheet(source.textContent, scope);
      if (rules === '') return undefined;
      target.append(page.createTextNode(rules));
      return target;
    }
    if (name === 'use' && !target.hasAttribute('href')) return undefined;
    copyChildren(source, target, depth);
    return target;
  }

  function copyChildren(source: Element, target: ParentNode, depth: number): void {
    for (const child of Array.from(source.childNodes)) {
      if (child.nodeType === 3) target.append(page.createTextNode(child.nodeValue ?? ''));
      else if (child.nodeType === 1) {
        const copied = copy(child as Element, depth + 1);
        if (copied) target.append(copied);
      }
    }
  }

  const picture = copy(root, 1, true);
  if (seen.tooLarge || !picture) return undefined;
  const svgElement = picture as SVGSVGElement;
  cleanDanglingReferences(svgElement, rootId, scope, copiedIds);
  return svgElement;
}
