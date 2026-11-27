/**
 * The reader's regional settings: which time zone their days are in, which
 * day their week starts on, and how they write the hours of a time.
 *
 * Each setting is either `auto` (follow the browser and the language) or a
 * fixed choice. `resolveRegional` turns the settings into the plain values
 * the rest of the logic takes as arguments. This module does not read the
 * browser itself: the caller passes what the browser says.
 *
 * The settings are the reader's account settings, and their values are
 * written the way the server stores and sends them (`settings` of the user):
 * `timeZone` is `auto` or an IANA name, `weekStart` is `auto`, `sunday`,
 * `monday` or `saturday`, `hourCycle` is `auto`, `h12` or `h23`. The server
 * keeps `auto` as it is and never resolves it.
 */
import type { HourCycle } from './format.ts';

/** A day of the week as ISO 8601 numbers it: 1 is Monday, 7 is Sunday. */
export type Weekday = 1 | 2 | 3 | 4 | 5 | 6 | 7;

/** The days a reader can fix the start of the week to; calendars use no other. */
export const WEEK_START_DAYS = { sunday: 7, monday: 1, saturday: 6 } as const satisfies Record<
  string,
  Weekday
>;

export type WeekStartSetting = 'auto' | keyof typeof WEEK_START_DAYS;
export type HourCycleSetting = 'auto' | HourCycle;

export const WEEK_START_SETTINGS: readonly WeekStartSetting[] = [
  'auto',
  'sunday',
  'monday',
  'saturday',
];
export const HOUR_CYCLE_SETTINGS: readonly HourCycleSetting[] = ['auto', 'h12', 'h23'];

export interface RegionalSettings {
  /** `auto` (the time zone of the browser), or an IANA name such as `Asia/Seoul`. */
  timeZone: string;
  /** `auto` (what is usual where the language is spoken), or a fixed day. */
  weekStart: WeekStartSetting;
  /** `auto` (what is usual in the language), or a fixed way of writing hours. */
  hourCycle: HourCycleSetting;
}

export const DEFAULT_REGIONAL_SETTINGS: RegionalSettings = {
  timeZone: 'auto',
  weekStart: 'auto',
  hourCycle: 'auto',
};

/**
 * The settings out of what the server sent. A member that is missing or holds
 * a value this version does not know reads as `auto`, so that a newer server
 * cannot leave the reader without dates.
 */
export function readRegionalSettings(stored: unknown): RegionalSettings {
  const source = (typeof stored === 'object' && stored !== null ? stored : {}) as Record<
    string,
    unknown
  >;
  const { timeZone, weekStart, hourCycle } = source;
  return {
    timeZone: typeof timeZone === 'string' && timeZone !== '' ? timeZone : 'auto',
    weekStart: WEEK_START_SETTINGS.find((known) => known === weekStart) ?? 'auto',
    hourCycle: HOUR_CYCLE_SETTINGS.find((known) => known === hourCycle) ?? 'auto',
  };
}

/** The settings with nothing left to look up. */
export interface Regional {
  timeZone: string;
  weekStart: Weekday;
  hourCycle: HourCycle;
}

/** Whether a name is a time zone this runtime knows. */
export function isTimeZone(name: string): boolean {
  try {
    new Intl.DateTimeFormat('en', { timeZone: name });
    return true;
  } catch {
    return false;
  }
}

/** The day weeks start on where `locale` is spoken; Monday when the runtime cannot say. */
export function weekStartOf(locale: string): Weekday {
  try {
    const tag = new Intl.Locale(locale) as Intl.Locale & {
      getWeekInfo?: () => { firstDay: number };
      weekInfo?: { firstDay: number };
    };
    const firstDay = tag.getWeekInfo?.().firstDay ?? tag.weekInfo?.firstDay;
    if (firstDay !== undefined && firstDay >= 1 && firstDay <= 7) return firstDay as Weekday;
  } catch {
    // Not a language tag: fall through to the default.
  }
  return 1;
}

/** Whether `locale` writes times with a.m. and p.m. or from 0 to 23. */
export function hourCycleOf(locale: string): HourCycle {
  try {
    const cycle = new Intl.DateTimeFormat(locale, {
      hour: 'numeric',
      timeZone: 'UTC',
    }).resolvedOptions().hourCycle;
    return cycle === 'h11' || cycle === 'h12' ? 'h12' : 'h23';
  } catch {
    return 'h23';
  }
}

/**
 * The values to work with. `browserTimeZone` is what the browser reports; it
 * is used for `auto`, and also when the fixed zone is one this runtime does
 * not know (a zone renamed since the setting was saved), so that dates still
 * show.
 */
export function resolveRegional(
  settings: RegionalSettings,
  locale: string,
  browserTimeZone: string,
): Regional {
  const fixed = settings.timeZone !== 'auto' && isTimeZone(settings.timeZone);
  return {
    timeZone: fixed ? settings.timeZone : isTimeZone(browserTimeZone) ? browserTimeZone : 'UTC',
    weekStart:
      settings.weekStart === 'auto' ? weekStartOf(locale) : WEEK_START_DAYS[settings.weekStart],
    hourCycle: settings.hourCycle === 'auto' ? hourCycleOf(locale) : settings.hourCycle,
  };
}
