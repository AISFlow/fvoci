/**
 * A cell's number as the viewer draws it through its format
 * (src/viewer-frame/xlsx-format.ts): digits, thousands and decimals rounded
 * as Excel rounds, percent and scaling, literals and currencies, sections
 * and conditions, colours, dates and times of both date systems, elapsed
 * times, and codes that cannot be read.
 */
import { describe, expect, it } from 'vitest';

import { readCode } from '../../src/viewer-frame/xlsx-codes.ts';
import { serialOf } from '../../src/viewer-frame/xlsx-dates.ts';
import { decimalParts, general } from '../../src/viewer-frame/xlsx-digits.ts';
import { codeOf, NumberFormat } from '../../src/viewer-frame/xlsx-format.ts';

const PALETTE = Array.from(
  { length: 64 },
  (_, index) => `#0000${index.toString(16).padStart(2, '0')}`,
);

/** The text of `value` in `code`, in the 1900 system, for a Korean reader unless told. */
function drawn(code: string, value: number, system: 1900 | 1904 = 1900, language = 'ko'): string {
  return new NumberFormat(code, system, language, PALETTE).number(value).text;
}

describe('digits', () => {
  it.each([
    ['0', 1234.5, '1235'],
    ['0', -1234.5, '-1235'],
    ['0.00', 2.005, '2.01'],
    ['0.00', 1.005, '1.01'],
    ['#,##0', 1234567.891, '1,234,568'],
    ['#,##0.00', 0.5, '0.50'],
    ['#,##0', 0, '0'],
    ['#', 0, ''],
    ['000', 7, '007'],
    ['?.??', 1.5, '1.5 '],
    ['#.##', 1.5, '1.5'],
    ['0.0%', 0.4567, '45.7%'],
    ['0%', -0.03, '-3%'],
    ['#,##0,', 1234567, '1,235'],
    ['0.0,,"M"', 1234567, '1.2M'],
    ['0.00E+00', 12345, '1.23E+04'],
    ['0.00E+00', 0.00012, '1.20E-04'],
    ['##0.0E+0', 12345, '12.3E+3'],
    ['0.00', -0.001, '-0.00'],
    ['"Q1 "0.00', -0.001, '-Q1 0.00'],
    ['0%', -0.004, '-0%'],
  ])('%s draws %d as %s', (code, value, text) => {
    expect(drawn(code, value)).toBe(text);
  });

  it('rounds to fifteen significant digits, halves away from zero', () => {
    expect(decimalParts(0.1 + 0.2, 2)).toEqual({ whole: '', fraction: '30' });
    expect(decimalParts(2.675, 2)).toEqual({ whole: '2', fraction: '68' });
    expect(decimalParts(2.5, 0)).toEqual({ whole: '3', fraction: '' });
    expect(decimalParts(3.5, 0)).toEqual({ whole: '4', fraction: '' });
  });

  it('General: up to eleven characters, then scientific', () => {
    expect(general(1234567.891)).toBe('1234567.891');
    expect(general(0.1 + 0.2)).toBe('0.3');
    expect(general(123456789012)).toBe('1.23457E+11');
    expect(general(0.00000000012)).toBe('1.2E-10');
    expect(general(-42)).toBe('-42');
    expect(drawn('General', 1 / 3)).toBe('0.333333333');
  });
});

describe('literals, currencies, colours and sections', () => {
  it.each([
    ['[$₩-412]#,##0', 1234567.891, '₩1,234,568'],
    ['"$"#,##0.00', 5, '$5.00'],
    ['#,##0"원"', 15000, '15,000원'],
    ['\\(0\\)', 3, '(3)'],
    ['0_);(0)', -3, '(3)'],
    ['0_);(0)', 3, '3 '],
    ['0;-0;"영"', 0, '영'],
    ['0;"음수"', -5, '음수'],
    ['[>=100]"많음";[<0]"음";"적음"', 150, '많음'],
    ['[>=100]"많음";[<0]"음";"적음"', -1, '음'],
    ['[>=100]"많음";[<0]"음";"적음"', 50, '적음'],
    ['_(* #,##0_);_(* (#,##0);_(* "-"_);_(@_)', 0, ' - '],
    ['_(* #,##0_);_(* (#,##0);_(* "-"_);_(@_)', -1234, ' (1,234)'],
  ])('%s draws %d as %s', (code, value, text) => {
    expect(drawn(code, value)).toBe(text);
  });

  it('a section names its colour, by name or from the palette', () => {
    const red = new NumberFormat('0;[Red]-0', 1900, 'ko', PALETTE).number(-2);
    expect(red).toEqual({ text: '-2', color: '#ff0000' });
    const eighth = new NumberFormat('[Color8]0', 1900, 'ko', PALETTE).number(1);
    expect(eighth.color).toBe(PALETTE[15]);
  });

  it('a text is drawn through the text section, and a number is not', () => {
    const format = new NumberFormat('0;-0;0;"[" @ "]"', 1900, 'ko', PALETTE);
    expect(format.text('값')).toEqual({ text: '[ 값 ]' });
    expect(new NumberFormat('@', 1900, 'ko', PALETTE).number(12.5).text).toBe('12.5');
  });

  it('a code that cannot be read, or one too long, draws the value as it is', () => {
    expect(drawn('"unclosed', 12.5)).toBe('12.5');
    expect(drawn('0'.repeat(300), 12.5)).toBe('12.5');
    expect(readCode('0;0;0;@;0;0')).toHaveLength(4);
    expect(drawn('# ?/?', 1.5)).toBe('1.5');
  });

  it('the built-in formats by number, the short date as the reader writes it', () => {
    const none = new Map<number, string>();
    expect(codeOf(3, none, 'ko')).toBe('#,##0');
    expect(codeOf(14, none, 'ko')).toBe('yyyy-mm-dd');
    expect(codeOf(14, none, 'en')).toBe('m/d/yyyy');
    expect(codeOf(22, none, 'ko')).toBe('yyyy-mm-dd h:mm');
    expect(codeOf(164, new Map([[164, '0.0']]), 'ko')).toBe('0.0');
    expect(codeOf(999, none, 'ko')).toBe('General');
  });
});

describe('dates and times', () => {
  const day = serialOf('2026-11-18', 1900) ?? 0;

  it.each([
    ['yyyy-mm-dd', day, '2026-11-18'],
    ['yyyy"년" m"월" d"일"', day, '2026년 11월 18일'],
    ['yy.m.d', day, '26.11.18'],
    ['mmm d, yyyy', day, 'Nov 18, 2026'],
    ['dddd', day, 'Wednesday'],
    ['aaa', day, '수'],
    ['h:mm', day + 0.5 + 1 / 48, '12:30'],
    ['hh:mm:ss', day + 0.75, '18:00:00'],
    ['h:mm AM/PM', day + 0.75, '6:00 PM'],
    ['[$-412]AM/PM h:mm', day + 0.75, '오후 6:00'],
    ['h:mm:ss.00', day + 0.5 + 0.256 / 86_400, '12:00:00.26'],
    ['[h]:mm', 1.5, '36:00'],
    ['[mm]:ss', 1 / 24, '60:00'],
    ['mm:ss', 1 / 1440 + 30 / 86_400, '01:30'],
  ])('%s draws %d as %s', (code, value, text) => {
    expect(drawn(code, value)).toBe(text);
  });

  it('m after an hour or before a second is minutes; elsewhere months', () => {
    expect(drawn('m', day)).toBe('11');
    expect(drawn('h:m', day + 0.5 + 5 / 1440)).toBe('12:5');
    expect(drawn('m:ss', day + 5 / 1440)).toBe('5:00');
  });

  it('a time that rounds to midnight goes on to the next day', () => {
    const almost = day + 1 - 0.4 / 86_400;
    expect(drawn('yyyy-mm-dd hh:mm:ss', almost)).toBe('2026-11-19 00:00:00');
    expect(drawn('yyyy-mm-dd hh:mm:ss.0', almost)).toBe('2026-11-18 23:59:59.6');
  });

  it('the 1900 system has its 29 February, and its days before it are a day off', () => {
    expect(drawn('yyyy-mm-dd', 59)).toBe('1900-02-28');
    expect(drawn('yyyy-mm-dd', 60)).toBe('1900-02-29');
    expect(drawn('yyyy-mm-dd', 61)).toBe('1900-03-01');
    expect(drawn('yyyy-mm-dd', 1)).toBe('1900-01-01');
    expect(drawn('ddd', 1, 1900, 'en')).toBe('Sun');
    expect(drawn('yyyy-mm-dd', 0)).toBe('1900-01-00');
  });

  it('the 1904 system counts from 1904, 1,462 days later', () => {
    expect(drawn('yyyy-mm-dd', 0, 1904)).toBe('1904-01-01');
    expect(drawn('yyyy-mm-dd', day - 1462, 1904)).toBe('2026-11-18');
    expect(drawn('ddd', 0, 1904, 'en')).toBe('Fri');
    expect(serialOf('2026-11-18', 1904)).toBe(day - 1462);
  });

  it('a date before the first day or past the last is hashes, as in Excel', () => {
    expect(drawn('yyyy-mm-dd', -1)).toBe('########');
    expect(drawn('yyyy-mm-dd', 2_958_466)).toBe('########');
    expect(drawn('[h]:mm', -1)).toBe('########');
  });

  it('the names of months and days in English, or in the locale the code names', () => {
    expect(drawn('mmmm', day, 1900, 'ko')).toBe('November');
    expect(drawn('[$-412]mmmm', day, 1900, 'en')).toBe('11월');
    expect(drawn('[$-412]dddd', day, 1900, 'en')).toBe('수요일');
    expect(drawn('[$-409]mmm', day, 1900, 'ko')).toBe('Nov');
    // `aaa` is the weekday as the reader says it.
    expect(drawn('aaaa', day, 1900, 'ko')).toBe('수요일');
    expect(drawn('[$-412]AM/PM h:mm', day + 0.25, 1900, 'en')).toBe('오전 6:00');
  });

  it('a stored date (`t="d"`) as its serial, in either system', () => {
    expect(serialOf('1900-03-01', 1900)).toBe(61);
    expect(serialOf('2026-11-18T12:00:00', 1900)).toBe(day + 0.5);
    expect(serialOf('not a date', 1900)).toBeUndefined();
  });
});
