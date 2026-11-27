/**
 * Draws formulas with KaTeX.
 *
 * The library and its style sheet are fetched the first time a formula has
 * to be drawn, never before: a body without formulas costs nothing.
 *
 * A formula is text one user wrote, drawn on the screen of another. So the
 * library runs with the settings it documents for input that is not trusted
 * (its docs/security.md and docs/options.md, version 0.19.1), and the limits
 * below bound what a single formula can cost.
 */
import type { KatexOptions } from 'katex';

import { MAX_LATEX } from './schema.ts';

/**
 * How KaTeX is run. Every setting here is about not trusting the source.
 *
 * - `trust: false`: the commands that make links, fetch pictures or set
 *   attributes of the page (`\href`, `\url`, `\includegraphics`,
 *   `\htmlClass`, `\htmlId`, `\htmlStyle`, `\htmlData`) are not carried out.
 * - `strict`: the HTML extension commands are an error outright, so a formula
 *   that uses one is not drawn at all. Everything else the library merely
 *   warns about (Korean text in a formula, say) is let through in silence.
 * - `maxExpand`: how many macro expansions one formula gets. This is what
 *   stops `\def\a{\a\a}\a` and the like. 1000 is the library's own default,
 *   said here so that a change of the default cannot loosen it.
 * - `maxSize`: the longest length a formula may ask for, in ems, so that
 *   `\rule{9999em}{9999em}` cannot cover the page.
 * - `throwOnError`: a formula that cannot be read throws, and the caller
 *   shows its own notice in place of the library's red source text.
 * - `errorColor`: what the library still paints itself (the name of a
 *   command it refused) takes the app's colour for errors.
 * - `output`: the drawing for the eye and MathML for a screen reader.
 * - `globalGroup: false` and a fresh `macros` object for every formula (see
 *   `drawMath`): a definition in one formula does not reach the next.
 */
export const KATEX_OPTIONS = {
  trust: false,
  strict: (code: string) => (code === 'htmlExtension' ? 'error' : 'ignore'),
  maxExpand: 1000,
  maxSize: 20,
  throwOnError: true,
  errorColor: 'var(--destructive)',
  output: 'htmlAndMathml',
  globalGroup: false,
} as const satisfies KatexOptions;

/** The part of the library that is used: it builds DOM nodes inside an element. */
export interface MathLibrary {
  render: (latex: string, element: HTMLElement, options?: KatexOptions) => void;
}

export type MathResult =
  /** `ms` is how long the library took, for whoever keeps a budget. */
  | { ok: true; ms: number }
  | { ok: false; reason: 'too-long' | 'invalid' | 'too-large'; ms: number };

/** How deep, and how many, the elements of one drawn formula may be. See `isTooLarge`. */
export const MAX_MATH_DEPTH = 200;
export const MAX_MATH_ELEMENTS = 20_000;

let loading: Promise<MathLibrary> | undefined;

/** The library, fetched on first use together with its style sheet. */
export function loadMath(): Promise<MathLibrary> {
  loading ??= Promise.all([import('katex'), import('./katex.css')]).then(
    ([library]) => library.default,
    (error: unknown) => {
      // A failed fetch is tried again the next time a formula comes up.
      loading = undefined;
      throw error;
    },
  );
  return loading;
}

const now = (): number => (typeof performance === 'undefined' ? Date.now() : performance.now());

/**
 * Draws a formula into `element`, in place of whatever it holds. When the
 * formula cannot be drawn the element is left exactly as it was.
 *
 * The library is given an element of its own to build into and only DOM
 * nodes come out of it (its `render`; the variant that returns markup is never used).
 */
export function drawMath(
  library: MathLibrary,
  element: HTMLElement,
  latex: string,
  display: boolean,
): MathResult {
  if (latex.length > MAX_LATEX) return { ok: false, reason: 'too-long', ms: 0 };
  const started = now();
  const drawn = element.ownerDocument.createElement('span');
  drawn.setAttribute('class', 'doc-math-drawn');
  try {
    library.render(latex, drawn, { ...KATEX_OPTIONS, displayMode: display, macros: {} });
  } catch {
    return { ok: false, reason: 'invalid', ms: now() - started };
  }
  // Checked before it goes into the page, which is where the damage would be done.
  if (isTooLarge(drawn)) return { ok: false, reason: 'too-large', ms: now() - started };
  element.replaceChildren(drawn);
  return { ok: true, ms: now() - started };
}

/**
 * Whether what the library built is more than a browser should be asked to
 * lay out. The library builds some six to eight elements inside each other
 * for every level of nesting in the source (a root in a root, a box in a
 * box), and a browser's layout slows down far faster than the depth grows:
 * measured in Chromium, 150 boxes in each other (1,200 elements deep) took
 * 3.4 seconds to lay out, 300 took 27, and 570 roots crashed the page. All
 * of those fit the limit on the source. Nothing a person writes comes near
 * the limit here: ten fractions in each other are 77 deep.
 */
function isTooLarge(root: Element): boolean {
  let count = 0;
  const stack: [Element, number][] = [[root, 1]];
  for (let next = stack.pop(); next !== undefined; next = stack.pop()) {
    const [element, depth] = next;
    count += 1;
    if (depth > MAX_MATH_DEPTH || count > MAX_MATH_ELEMENTS) return true;
    for (const child of Array.from(element.children)) stack.push([child, depth + 1]);
  }
  return false;
}
