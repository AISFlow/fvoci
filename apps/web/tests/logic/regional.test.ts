/**
 * The reader's regional settings: time zone, first day of the week and the
 * way hours are written. Each is `auto` or a fixed choice.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { displayDateTime } from '$lib/logic/format.ts';
import {
  DEFAULT_REGIONAL_SETTINGS,
  HOUR_CYCLE_SETTINGS,
  WEEK_START_SETTINGS,
  hourCycleOf,
  isTimeZone,
  readRegionalSettings,
  resolveRegional,
  weekStartOf,
  type WeekStartSetting,
} from '$lib/logic/regional.ts';
import { browserTimeZone, regional, regionalSettings, setRegionalSettings } from '$lib/regional.ts';

describe('regional settings', () => {
  it('follow the browser and the language by default', () => {
    expect(DEFAULT_REGIONAL_SETTINGS).toEqual({
      timeZone: 'auto',
      weekStart: 'auto',
      hourCycle: 'auto',
    });
    expect(resolveRegional(DEFAULT_REGIONAL_SETTINGS, 'ko', 'Asia/Seoul')).toEqual({
      timeZone: 'Asia/Seoul',
      weekStart: 7,
      hourCycle: 'h12',
    });
    expect(resolveRegional(DEFAULT_REGIONAL_SETTINGS, 'de', 'Europe/Berlin')).toEqual({
      timeZone: 'Europe/Berlin',
      weekStart: 1,
      hourCycle: 'h23',
    });
  });

  it('keep a fixed choice whatever the browser and the language say', () => {
    const fixed = {
      timeZone: 'Pacific/Kiritimati',
      weekStart: 'saturday',
      hourCycle: 'h23',
    } as const;
    expect(resolveRegional(fixed, 'en-US', 'Asia/Seoul')).toEqual({
      timeZone: 'Pacific/Kiritimati',
      weekStart: 6,
      hourCycle: 'h23',
    });
  });

  it('are written the way the server stores them', () => {
    expect(WEEK_START_SETTINGS).toEqual(['auto', 'sunday', 'monday', 'saturday']);
    expect(HOUR_CYCLE_SETTINGS).toEqual(['auto', 'h12', 'h23']);
    const resolved = (weekStart: WeekStartSetting): number =>
      resolveRegional({ ...DEFAULT_REGIONAL_SETTINGS, weekStart }, 'de', 'UTC').weekStart;
    // ISO 8601 numbers the days from Monday (1) to Sunday (7).
    expect(resolved('sunday')).toBe(7);
    expect(resolved('monday')).toBe(1);
    expect(resolved('saturday')).toBe(6);
    expect(resolved('auto')).toBe(1);
  });

  it('are read from what the server sent, and anything unknown reads as auto', () => {
    expect(
      readRegionalSettings({ timeZone: 'Asia/Seoul', weekStart: 'monday', hourCycle: 'h23' }),
    ).toEqual({ timeZone: 'Asia/Seoul', weekStart: 'monday', hourCycle: 'h23' });
    expect(
      readRegionalSettings({ timeZone: 'auto', weekStart: 'auto', hourCycle: 'auto' }),
    ).toEqual(DEFAULT_REGIONAL_SETTINGS);
    for (const stored of [
      undefined,
      null,
      'auto',
      {},
      { timeZone: '', weekStart: 1, hourCycle: 'h24' },
      { timeZone: 9, weekStart: 'friday', hourCycle: true },
    ]) {
      expect(readRegionalSettings(stored)).toEqual(DEFAULT_REGIONAL_SETTINGS);
    }
  });

  it('fall back to the browser for a fixed time zone the runtime does not know', () => {
    const settings = { ...DEFAULT_REGIONAL_SETTINGS, timeZone: 'Mars/Olympus_Mons' };
    expect(resolveRegional(settings, 'en', 'Asia/Seoul').timeZone).toBe('Asia/Seoul');
    // And to UTC when the browser's answer is no better.
    expect(resolveRegional(settings, 'en', '').timeZone).toBe('UTC');
  });

  it('know a time zone name from something else', () => {
    expect(isTimeZone('Asia/Seoul')).toBe(true);
    expect(isTimeZone('UTC')).toBe(true);
    expect(isTimeZone('auto')).toBe(false);
    expect(isTimeZone('')).toBe(false);
  });

  it('know the first day of the week of a language and region', () => {
    expect(weekStartOf('en-US')).toBe(7);
    expect(weekStartOf('en-GB')).toBe(1);
    expect(weekStartOf('ko')).toBe(7);
    expect(weekStartOf('fr')).toBe(1);
    expect(weekStartOf('not a language')).toBe(1);
  });

  it('know how a language writes hours', () => {
    expect(hourCycleOf('en-US')).toBe('h12');
    expect(hourCycleOf('ko')).toBe('h12');
    expect(hourCycleOf('de')).toBe('h23');
    expect(hourCycleOf('ja')).toBe('h23');
  });

  it('decide how the hours of a time are shown', () => {
    const moment = Date.UTC(2026, 2, 5, 6, 20);
    const context = { locale: 'en', timeZone: 'Asia/Seoul', now: Date.UTC(2026, 5, 1) };
    expect(displayDateTime(moment, context)).toBe('March 05 at 3:20 PM');
    expect(displayDateTime(moment, { ...context, hourCycle: 'h12' })).toBe('March 05 at 3:20 PM');
    expect(displayDateTime(moment, { ...context, hourCycle: 'h23' })).toBe('March 05 at 15:20');
  });
});

describe('the place the app reads the settings from', () => {
  afterEach(() => {
    setRegionalSettings(DEFAULT_REGIONAL_SETTINGS);
  });

  it('uses the time zone of the browser until the reader fixes one', () => {
    expect(isTimeZone(browserTimeZone())).toBe(true);
    expect(regionalSettings()).toEqual(DEFAULT_REGIONAL_SETTINGS);
    expect(regional('ko').timeZone).toBe(browserTimeZone());
    // The test runner sets TZ; the browser's zone is the process's zone here.
    if (process.env.TZ) expect(browserTimeZone()).toBe(process.env.TZ);
  });

  it('uses the fixed choices once they are set', () => {
    setRegionalSettings({ timeZone: 'America/New_York', weekStart: 'monday', hourCycle: 'h23' });
    expect(regional('ko')).toEqual({
      timeZone: 'America/New_York',
      weekStart: 1,
      hourCycle: 'h23',
    });
  });
});
