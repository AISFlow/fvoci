/**
 * The language the app is shown in.
 *
 * It is the reader's account setting (`language`): `auto` is the first
 * language of the browser that the app has messages for, or else English; a
 * tag is the message set closest to it. The account is not known before the
 * first answer, and the sign-in page has none, so the device keeps a copy of
 * the setting and the page starts in it; once the account's settings arrive
 * and say otherwise, the copy follows them and the page loads again.
 * Changing the language reloads the page too: every text on screen is a
 * message, and a reload is the simple way to have all of them again.
 */
import { AUTO_LANGUAGE } from '$lib/logic/language.ts';
import { getLocale, locales, setLocale } from '$lib/i18n/paraglide/runtime.js';

export type Locale = (typeof locales)[number];

/** Where the device keeps its copy of the account's setting; nothing there is `auto`. */
const STORAGE_KEY = 'fvoci:locale';

const isLocale = (tag: string | null | undefined): tag is Locale =>
  (locales as readonly string[]).includes(tag ?? '');

/** The message set closest to a language tag, if there is one. */
export function matchLocale(tag: string): Locale | undefined {
  if (isLocale(tag)) return tag;
  const lower = tag.toLowerCase();
  const exact = locales.find((locale) => locale.toLowerCase() === lower);
  if (exact) return exact;
  const language = lower.split('-')[0];
  return isLocale(language) ? language : undefined;
}

/** The message set the browser asks for: what `auto` means. */
export function browserLocale(): Locale {
  return navigator.languages.map(matchLocale).find((tag) => tag !== undefined) ?? 'en';
}

/** The message set a setting (`auto` or a tag) is shown in. */
export function localeOfSetting(setting: string): Locale {
  return (setting === AUTO_LANGUAGE ? undefined : matchLocale(setting)) ?? browserLocale();
}

function remembered(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) ?? AUTO_LANGUAGE;
  } catch {
    return AUTO_LANGUAGE;
  }
}

/** Keeps the setting on the device: `auto` is no copy at all. */
function remember(setting: string): void {
  try {
    if (setting === AUTO_LANGUAGE) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, setting);
  } catch {
    // Not kept: the page starts in the browser's language and follows the account.
  }
}

/** Settles the language for this visit and tells the document. Call before anything is drawn. */
export function startLocale(): Locale {
  const locale = localeOfSetting(remembered());
  void setLocale(locale, { reload: false });
  document.documentElement.lang = locale;
  return locale;
}

export function currentLocale(): Locale {
  return getLocale();
}

/**
 * Follows the account's setting once it is read: the device keeps it, and
 * when the page is in another language it loads again in this one. Answers
 * whether it does. A device that cannot keep the setting would start the
 * next page in the language of before, and follow again, and again: it
 * stays in the language it is in.
 */
export function followAccountLanguage(setting: string): boolean {
  remember(setting);
  const locale = localeOfSetting(setting);
  if (locale === getLocale() || remembered() !== setting) return false;
  void setLocale(locale, { reload: true });
  return true;
}

/**
 * What the language picker shows as chosen: the language the page is in,
 * or `auto` when the account says so and the page is in the browser's. A
 * setting changed on another device since the page was loaded is not shown
 * until the page follows it, so that it can be chosen here again.
 */
export function shownLanguage(setting: string): string {
  const locale = getLocale();
  return setting === AUTO_LANGUAGE && locale === browserLocale() ? AUTO_LANGUAGE : locale;
}

export { locales };
