/**
 * Editing the value of a date property: days, typed dates, times of day on
 * the reader's clock, and the two kinds of time that clocks which change
 * make ambiguous.
 *
 * Every case names its time zone; nothing here depends on the time zone of
 * the machine (the suite runs under three of them).
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import {
  dayPeriods,
  fromTwelveHour,
  hasTime,
  hourChoices,
  instantOfWallTime,
  occurrenceOf,
  orderRange,
  parseTypedDate,
  toTwelveHour,
  typedDateText,
  wallTimeOf,
  withDay,
  withTime,
  withTimeOfDay,
  withoutTime,
} from '$lib/logic/date-entry.ts';
import { calendarDate, startOfDay, type CalendarDate } from '$lib/logic/dates.ts';
import { formatDateValue } from '$lib/logic/wire.ts';

import { at, ZONES } from '../support/zones.ts';

const HOUR = 3_600_000;
const day = (text: string): CalendarDate => text as CalendarDate;

describe('the clock of a time zone', () => {
  it('reads a moment as the date and time the reader sees', () => {
    const moment = Date.UTC(2026, 2, 5, 0, 30);
    expect(wallTimeOf(moment, 'Asia/Seoul')).toEqual({ date: '2026-03-05', hour: 9, minute: 30 });
    expect(wallTimeOf(moment, 'America/New_York')).toEqual({
      date: '2026-03-04',
      hour: 19,
      minute: 30,
    });
    expect(wallTimeOf(moment, 'Asia/Kathmandu')).toEqual({
      date: '2026-03-05',
      hour: 6,
      minute: 15,
    });
  });

  it('finds the moment of a date and time on that clock', () => {
    expect(instantOfWallTime({ date: day('2026-03-05'), hour: 9, minute: 30 }, 'Asia/Seoul')).toBe(
      Date.UTC(2026, 2, 5, 0, 30),
    );
    expect(
      instantOfWallTime({ date: day('2026-03-04'), hour: 19, minute: 30 }, 'America/New_York'),
    ).toBe(Date.UTC(2026, 2, 5, 0, 30));
  });

  it('gives back the time it was asked for, in every zone, when that time exists once', () => {
    fc.assert(
      fc.property(
        fc.constantFrom(...ZONES),
        fc.integer({ min: 2000, max: 2040 }),
        fc.integer({ min: 1, max: 12 }),
        fc.integer({ min: 1, max: 28 }),
        fc.integer({ min: 0, max: 23 }),
        fc.integer({ min: 0, max: 59 }),
        (zone, year, month, dayOfMonth, hour, minute) => {
          const wall = { date: calendarDate(year, month, dayOfMonth), hour, minute };
          const moment = instantOfWallTime(wall, zone);
          const shown = wallTimeOf(moment, zone);
          // A time the clocks skip is read back as the time they jumped to,
          // later on the same day and never before the day starts (which is
          // that time itself when the skipped time is midnight); every other
          // time is read back as it was entered.
          expect(shown.date).toBe(wall.date);
          if (shown.hour !== hour || shown.minute !== minute) {
            expect(shown.hour * 60 + shown.minute).toBeGreaterThan(hour * 60 + minute);
            expect(moment).toBeGreaterThanOrEqual(startOfDay(wall.date, zone));
          }
          // And the moment always reads back as itself.
          expect(instantOfWallTime(shown, zone, occurrenceOf(moment, zone))).toBe(moment);
        },
      ),
      { numRuns: 500 },
    );
  });
});

describe('a time the clocks skip', () => {
  // New York, 8 March 2026: at 02:00 the clocks go to 03:00.
  const zone = 'America/New_York';

  it('becomes the time that far past the skip', () => {
    const asked = instantOfWallTime({ date: day('2026-03-08'), hour: 2, minute: 30 }, zone);
    expect(asked).toBe(at(zone, 2026, 3, 8, 3, 30));
    expect(wallTimeOf(asked, zone)).toEqual({ date: '2026-03-08', hour: 3, minute: 30 });
    // One and a half hours after 01:00, not two and a half.
    expect(asked - at(zone, 2026, 3, 8, 1, 0)).toBe(1.5 * HOUR);
  });

  it('is taken the same way whichever of two moments is preferred', () => {
    const wall = { date: day('2026-03-08'), hour: 2, minute: 30 };
    expect(instantOfWallTime(wall, zone, 'later')).toBe(instantOfWallTime(wall, zone, 'earlier'));
  });

  it('is where a value lands when it is moved onto that day at that time', () => {
    const before = { instant: at(zone, 2026, 3, 7, 2, 30) };
    expect(withDay(before, day('2026-03-08'), zone)).toEqual({
      instant: at(zone, 2026, 3, 8, 3, 30),
    });
    expect(withTimeOfDay(at(zone, 2026, 3, 8, 12, 0), 2, 15, zone)).toBe(
      at(zone, 2026, 3, 8, 3, 15),
    );
  });

  it('is skipped by half an hour where the clocks change by half an hour', () => {
    // Lord Howe Island, 4 October 2026: at 02:00 the clocks go to 02:30.
    const howe = 'Australia/Lord_Howe';
    const asked = instantOfWallTime({ date: day('2026-10-04'), hour: 2, minute: 10 }, howe);
    expect(wallTimeOf(asked, howe)).toEqual({ date: '2026-10-04', hour: 2, minute: 40 });
  });

  it('is the start of the day when it is midnight itself', () => {
    // Havana, 8 March 2015: the clocks went from 00:00 to 01:00. Midnight was
    // asked for, and the day's first moment, 01:00, is what is given: the
    // same moment the day starts at, not a later one.
    const havana = 'America/Havana';
    const asked = instantOfWallTime({ date: day('2015-03-08'), hour: 0, minute: 0 }, havana);
    expect(asked).toBe(Date.UTC(2015, 2, 8, 5, 0));
    expect(wallTimeOf(asked, havana)).toEqual({ date: '2015-03-08', hour: 1, minute: 0 });
    expect(asked).toBe(startOfDay(day('2015-03-08'), havana));
  });

  it('can be midnight itself', () => {
    // Havana, 8 March 2026: midnight does not exist, the day starts at 01:00.
    const havana = 'America/Havana';
    const asked = instantOfWallTime({ date: day('2026-03-08'), hour: 0, minute: 20 }, havana);
    expect(wallTimeOf(asked, havana)).toEqual({ date: '2026-03-08', hour: 1, minute: 20 });
  });
});

describe('a time that happens twice', () => {
  // New York, 1 November 2026: at 02:00 the clocks go back to 01:00.
  const zone = 'America/New_York';
  const wall = { date: day('2026-11-01'), hour: 1, minute: 30 };
  const first = Date.UTC(2026, 10, 1, 5, 30);
  const second = Date.UTC(2026, 10, 1, 6, 30);

  it('means the first of the two when a person enters it', () => {
    expect(instantOfWallTime(wall, zone)).toBe(first);
    expect(instantOfWallTime(wall, zone, 'later')).toBe(second);
  });

  it('reads the same on the clock both times', () => {
    expect(wallTimeOf(first, zone)).toEqual(wall);
    expect(wallTimeOf(second, zone)).toEqual(wall);
    expect(occurrenceOf(first, zone)).toBe('earlier');
    expect(occurrenceOf(second, zone)).toBe('later');
  });

  it('stays the second one while the minute changes within the repeated hour', () => {
    expect(withTimeOfDay(second, 1, 45, zone)).toBe(second + 15 * 60_000);
    expect(withTimeOfDay(first, 1, 45, zone)).toBe(first + 15 * 60_000);
  });

  it('is an ordinary time again once it leaves the repeated hour', () => {
    expect(withTimeOfDay(second, 3, 0, zone)).toBe(at(zone, 2026, 11, 1, 3, 0));
    expect(withDay({ instant: second }, day('2026-11-02'), zone)).toEqual({
      instant: at(zone, 2026, 11, 2, 1, 30),
    });
  });

  it('calls a time that happens once the earlier one', () => {
    expect(occurrenceOf(at(zone, 2026, 6, 1, 1, 30), zone)).toBe('earlier');
    expect(occurrenceOf(at('Asia/Seoul', 2026, 11, 1, 1, 30), 'Asia/Seoul')).toBe('earlier');
  });
});

describe('moving one end to another day', () => {
  it('replaces a day by the day', () => {
    expect(withDay({ date: day('2026-03-05') }, day('2026-04-01'), 'Asia/Seoul')).toEqual({
      date: '2026-04-01',
    });
  });

  it('keeps the time of day of a moment, on the clock of the reader', () => {
    const moment = { instant: at('America/New_York', 2026, 3, 5, 14, 30) };
    expect(withDay(moment, day('2026-07-01'), 'America/New_York')).toEqual({
      instant: at('America/New_York', 2026, 7, 1, 14, 30),
    });
    // The same moment is 04:30 on the 6th in Seoul; a reader there who picks
    // the 10th gets 04:30 on the 10th.
    expect(withDay(moment, day('2026-03-10'), 'Asia/Seoul')).toEqual({
      instant: at('Asia/Seoul', 2026, 3, 10, 4, 30),
    });
  });

  it('keeps the seconds out: a moment is entered to the minute', () => {
    const moment = at('Asia/Seoul', 2026, 3, 5, 9, 0, 42);
    expect(withTimeOfDay(moment, 10, 5, 'Asia/Seoul')).toBe(at('Asia/Seoul', 2026, 3, 5, 10, 5));
  });
});

describe('turning the time on and off', () => {
  const zone = 'America/New_York';

  it('puts a day at nine in the morning on the reader’s clock', () => {
    expect(withTime({ from: { date: day('2026-03-05') } }, zone)).toEqual({
      from: { instant: at(zone, 2026, 3, 5, 9, 0) },
    });
    expect(withTime({ from: { date: day('2026-03-05') } }, 'Asia/Seoul')).toEqual({
      from: { instant: at('Asia/Seoul', 2026, 3, 5, 9, 0) },
    });
  });

  it('puts both ends of a range at nine', () => {
    expect(
      withTime({ from: { date: day('2026-03-05') }, to: { date: day('2026-03-09') } }, zone),
    ).toEqual({
      from: { instant: at(zone, 2026, 3, 5, 9, 0) },
      to: { instant: at(zone, 2026, 3, 9, 9, 0) },
    });
  });

  it('leaves an empty value empty and a value with a time as it is', () => {
    expect(withTime({}, zone)).toEqual({});
    const timed = { from: { instant: at(zone, 2026, 3, 5, 17, 45) } };
    expect(withTime(timed, zone)).toEqual(timed);
    expect(hasTime(timed)).toBe(true);
    expect(hasTime({ from: { date: day('2026-03-05') } })).toBe(false);
    expect(hasTime({})).toBe(false);
  });

  it('turns each moment back into the day it falls on for the reader', () => {
    const value = {
      from: { instant: at('Asia/Seoul', 2026, 3, 5, 9, 0) },
      to: { instant: at('Asia/Seoul', 2026, 3, 9, 9, 0) },
    };
    expect(withoutTime(value, 'Asia/Seoul')).toEqual({
      from: { date: '2026-03-05' },
      to: { date: '2026-03-09' },
    });
    // 09:00 in Seoul is the evening before in New York.
    expect(withoutTime(value, 'America/New_York')).toEqual({
      from: { date: '2026-03-04' },
      to: { date: '2026-03-08' },
    });
    expect(withoutTime({}, 'Asia/Seoul')).toEqual({});
  });

  it('writes a range of two moments on one day as that one day', () => {
    const value = {
      from: { instant: at(zone, 2026, 3, 5, 9, 0) },
      to: { instant: at(zone, 2026, 3, 5, 17, 0) },
    };
    expect(formatDateValue(withoutTime(value, zone))).toEqual({ from: '2026-03-05' });
  });
});

describe('a range after one end was edited', () => {
  const fifth = { date: day('2026-03-05') };
  const ninth = { date: day('2026-03-09') };

  it('is left alone when it runs forwards', () => {
    expect(orderRange({ from: fifth, to: ninth }, 'from')).toEqual({ from: fifth, to: ninth });
    expect(orderRange({ from: fifth }, 'from')).toEqual({ from: fifth });
    expect(orderRange({}, 'to')).toEqual({});
  });

  it('pulls the end to a start that was moved past it', () => {
    expect(orderRange({ from: ninth, to: fifth }, 'from')).toEqual({ from: ninth, to: ninth });
  });

  it('pulls the start to an end that was moved before it', () => {
    expect(orderRange({ from: ninth, to: fifth }, 'to')).toEqual({ from: fifth, to: fifth });
  });

  it('does the same for moments', () => {
    const nine = { instant: at('Asia/Seoul', 2026, 3, 5, 9, 0) };
    const eight = { instant: at('Asia/Seoul', 2026, 3, 5, 8, 0) };
    expect(orderRange({ from: nine, to: eight }, 'to')).toEqual({ from: eight, to: eight });
    expect(formatDateValue(orderRange({ from: nine, to: eight }, 'to'))).toEqual({
      from: eight.instant,
    });
  });

  it('keeps only the start of a value whose ends are of two kinds', () => {
    const nine = { instant: at('Asia/Seoul', 2026, 3, 5, 9, 0) };
    expect(orderRange({ from: fifth, to: nine }, 'to')).toEqual({ from: fifth });
  });
});

describe('a typed date', () => {
  it('is written in the order of the language', () => {
    expect(typedDateText(day('2026-03-05'), 'en-US')).toBe('03/05/2026');
    expect(typedDateText(day('2026-03-05'), 'es')).toBe('05/03/2026');
    expect(typedDateText(day('2026-03-05'), 'de')).toBe('05.03.2026');
    expect(typedDateText(day('2026-03-05'), 'ko')).toBe('2026. 03. 05.');
    expect(typedDateText(day('0099-12-31'), 'en-US')).toContain('99');
  });

  it('is read in the order of the language', () => {
    expect(parseTypedDate('07/15/2021', 'en-US')).toBe('2021-07-15');
    expect(parseTypedDate('15/07/2021', 'es')).toBe('2021-07-15');
    expect(parseTypedDate('15.07.2021', 'de')).toBe('2021-07-15');
    expect(parseTypedDate('2021. 7. 15.', 'ko')).toBe('2021-07-15');
    expect(parseTypedDate('7-15-2021', 'en-US')).toBe('2021-07-15');
  });

  it('is read as year, month, day when it starts with the year', () => {
    expect(parseTypedDate('2021-07-15', 'en-US')).toBe('2021-07-15');
    expect(parseTypedDate('2021/07/15', 'es')).toBe('2021-07-15');
  });

  it('reads back what was written, in every language tried', () => {
    for (const locale of ['en-US', 'en-GB', 'es', 'de', 'fr', 'ko', 'ja', 'zh-Hans', 'sv', 'ru']) {
      for (const date of ['2026-03-05', '2024-02-29', '1999-12-31']) {
        expect([locale, parseTypedDate(typedDateText(day(date), locale), locale)]).toEqual([
          locale,
          date,
        ]);
      }
    }
  });

  it('is nothing when it is not a day that exists', () => {
    expect(parseTypedDate('02/30/2021', 'en-US')).toBeUndefined();
    expect(parseTypedDate('13/13/2021', 'en-US')).toBeUndefined();
    expect(parseTypedDate('tomorrow', 'en-US')).toBeUndefined();
    expect(parseTypedDate('07/15', 'en-US')).toBeUndefined();
    expect(parseTypedDate('07/15/21', 'en-US')).toBeUndefined();
    expect(parseTypedDate('', 'en-US')).toBeUndefined();
    expect(parseTypedDate('1/2/3/4', 'en-US')).toBeUndefined();
    expect(parseTypedDate('0000-01-01', 'en-US')).toBeUndefined();
  });
});

describe('the hours of a clock', () => {
  it('are 0 to 23, or 12 and then 1 to 11', () => {
    expect(hourChoices('h23')).toHaveLength(24);
    expect(hourChoices('h23')[0]).toBe(0);
    expect(hourChoices('h12')).toEqual([12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
  });

  it('name their halves of the day in the language, before or after the hour', () => {
    expect(dayPeriods('en-US')).toEqual({ am: 'AM', pm: 'PM', first: false });
    expect(dayPeriods('ko')).toEqual({ am: '오전', pm: '오후', first: true });
    // A language that counts to 23 still has names for the halves.
    expect(dayPeriods('de').first).toBe(false);
  });

  it('convert between the two ways of counting', () => {
    expect(toTwelveHour(0)).toEqual({ hour: 12, period: 'am' });
    expect(toTwelveHour(9)).toEqual({ hour: 9, period: 'am' });
    expect(toTwelveHour(12)).toEqual({ hour: 12, period: 'pm' });
    expect(toTwelveHour(23)).toEqual({ hour: 11, period: 'pm' });
    for (let hour = 0; hour < 24; hour += 1) {
      const shown = toTwelveHour(hour);
      expect(fromTwelveHour(shown.hour, shown.period)).toBe(hour);
    }
  });
});
