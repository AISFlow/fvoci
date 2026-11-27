/**
 * The language of the interface as an account setting (`language`): `auto`
 * follows the browser, anything else is a language tag in the shape of
 * Unicode BCP 47, as the server checks it. The server checks the shape only
 * and answers a tag as it was given; which messages a tag is shown with is
 * the app's to decide (`src/app/locale.ts`).
 */

/** The setting that follows the browser. */
export const AUTO_LANGUAGE = 'auto';

/** The longest tag the server takes. */
export const LANGUAGE_MAX_LENGTH = 64;

export interface LanguageSettings {
  /** `auto`, or a language tag such as `ko` or `pt-BR`. */
  language: string;
}

export const DEFAULT_LANGUAGE_SETTINGS: LanguageSettings = { language: AUTO_LANGUAGE };

const letters = (subtag: string, min: number, max: number): boolean =>
  subtag.length >= min && subtag.length <= max && /^[A-Za-z]+$/.test(subtag);
const alphanumerics = (subtag: string, min: number, max: number): boolean =>
  subtag.length >= min && subtag.length <= max && /^[A-Za-z0-9]+$/.test(subtag);
const isRegion = (subtag: string): boolean => letters(subtag, 2, 2) || /^[0-9]{3}$/.test(subtag);
const isVariant = (subtag: string): boolean =>
  alphanumerics(subtag, 5, 8) || /^[0-9][A-Za-z0-9]{3}$/.test(subtag);

/**
 * Whether a tag has the shape the server takes, subtag by subtag as its
 * check reads it (Unicode's `unicode_locale_id`): a language of 2–3 or 5–8
 * letters, then a script, a region, variants without repeats, extensions
 * each with its subtags, and private use last.
 */
function isLocaleTag(tag: string): boolean {
  const subtags = tag.split('-');
  let at = 0;
  const next = (wanted: (subtag: string) => boolean): string | undefined => {
    const subtag = subtags[at];
    if (subtag === undefined || !wanted(subtag)) return undefined;
    at += 1;
    return subtag;
  };
  if (!next((subtag) => letters(subtag, 2, 3) || letters(subtag, 5, 8))) return false;
  next((subtag) => letters(subtag, 4, 4));
  next(isRegion);
  const variants = new Set<string>();
  for (let variant = next(isVariant); variant !== undefined; variant = next(isVariant)) {
    if (variants.has(variant.toLowerCase())) return false;
    variants.add(variant.toLowerCase());
  }
  const singletons = new Set<string>();
  while (at < subtags.length) {
    const singleton = subtags[at] ?? '';
    at += 1;
    if (!alphanumerics(singleton, 1, 1) || singletons.has(singleton.toLowerCase())) return false;
    singletons.add(singleton.toLowerCase());
    const isPrivate = singleton.toLowerCase() === 'x';
    let any = false;
    while (next((subtag) => alphanumerics(subtag, isPrivate ? 1 : 2, 8))) any = true;
    if (!any) return false;
    // Private use is last: whatever follows it is part of it.
    if (isPrivate) return at === subtags.length;
  }
  return true;
}

/**
 * Whether the setting is one the server keeps: `auto`, or a language tag of
 * at most 64 characters in its shape. "Auto" or "en-US-US" is not.
 */
export function isLanguageSetting(value: string): boolean {
  if (value === AUTO_LANGUAGE) return true;
  return value.length <= LANGUAGE_MAX_LENGTH && isLocaleTag(value);
}

/** The setting out of what the server sent: one that is missing or has no shape reads as `auto`. */
export function readLanguageSettings(stored: unknown): LanguageSettings {
  const { language } = (typeof stored === 'object' && stored !== null ? stored : {}) as Record<
    string,
    unknown
  >;
  return {
    language:
      typeof language === 'string' && isLanguageSetting(language) ? language : AUTO_LANGUAGE,
  };
}
