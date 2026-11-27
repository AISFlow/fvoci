/**
 * Loading of the `@font-face` rules that tools/fonts/build.ts writes.
 *
 * The rules of a CJK family and of the monospace family list thousands of
 * characters, so they are not part of the main style sheet. The family of the
 * current language is requested right away, the others once the browser is
 * idle: text in another language still gets its own family, a moment later. A font file itself is only
 * downloaded when a character on the page needs it.
 *
 * When the fonts have not been built the globs below are empty and the page
 * falls back to the fonts of the system.
 */

/** The Latin family lists a handful of ranges: part of the main style sheet. */
import.meta.glob('../styles/fonts/faces-noto-sans.css', { eager: true });

const lazySheets = import.meta.glob<unknown>([
  '../styles/fonts/faces-*.css',
  '!../styles/fonts/faces-noto-sans.css',
]);

type Region = 'kr' | 'jp' | 'sc' | 'tc';

/** The family that draws the Han characters of a language tag, if it is a CJK language. */
export function regionOf(language: string): Region | undefined {
  const tag = language.toLowerCase().replaceAll('_', '-');
  if (tag === 'ko' || tag.startsWith('ko-')) return 'kr';
  if (tag === 'ja' || tag.startsWith('ja-')) return 'jp';
  if (tag === 'zh' || tag.startsWith('zh-')) {
    return /-(hant|tw|hk|mo)\b/.test(tag) ? 'tc' : 'sc';
  }
  return undefined;
}

function load(id: string): void {
  // A failed request leaves system fonts in place; there is nothing to recover.
  lazySheets[`../styles/fonts/faces-${id}.css`]?.().catch(() => undefined);
}

function whenIdle(task: () => void): void {
  if (typeof requestIdleCallback === 'function') requestIdleCallback(task, { timeout: 5000 });
  else setTimeout(task, 1500);
}

/** Requests the font rules, those of the family of `language` first. */
export function loadFontFaces(language: string): void {
  const first = regionOf(language);
  if (first) load(`noto-sans-${first}`);
  whenIdle(() => {
    for (const region of ['kr', 'jp', 'sc', 'tc'] as const) {
      if (region !== first) load(`noto-sans-${region}`);
    }
    load('d2coding');
  });
}
