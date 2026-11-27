/**
 * Where the app learns the reader's time zone, first day of the week and
 * way of writing hours.
 *
 * This is the only file that asks the browser for its time zone. Everything
 * else takes the time zone as an argument, and gets it from `regional()`.
 *
 * The settings will be part of the reader's account. Until the app talks to
 * a server they are the defaults: follow the browser and the language.
 */
import {
  DEFAULT_REGIONAL_SETTINGS,
  resolveRegional,
  type Regional,
  type RegionalSettings,
} from './logic/regional.ts';

/** The time zone the browser is set to, as an IANA name. */
export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

let settings: RegionalSettings = DEFAULT_REGIONAL_SETTINGS;

/** Replaces the settings: called when the reader's account settings arrive or change. */
export function setRegionalSettings(next: RegionalSettings): void {
  settings = next;
}

export function regionalSettings(): RegionalSettings {
  return settings;
}

/** The values to pass on, for a reader who reads the app in `locale`. */
export function regional(locale: string): Regional {
  return resolveRegional(settings, locale, browserTimeZone());
}
