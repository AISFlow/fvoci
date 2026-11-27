/** The calculations of the summary row: which a column can have, what they are called, what they give. */
import { describe, expect, it } from 'vitest';

import {
  calculationLabel,
  calculationOf,
  calculationOptions,
  calculationResult,
  calculationShortName,
  titleProperty,
} from '$lib/features/views/table/calculation-names.ts';
import { CALCULATION_IDS } from '$lib/logic/calculations.ts';
import type { DisplayContext } from '$lib/logic/format.ts';
import {
  createCard,
  TITLE_COLUMN_ID,
  type Card,
  type PropertyTemplate,
  type PropertyType,
} from '$lib/logic/wire.ts';

const property = (type: PropertyType): PropertyTemplate => ({
  id: 'p',
  name: 'Property',
  type,
  options: [],
});
const CONTEXT: DisplayContext = { locale: 'en', timeZone: 'UTC', hourCycle: 'h23' };

function card(value?: string): Card {
  const made = createCard();
  if (value !== undefined) made.fields.properties.p = value;
  return made;
}

describe('the calculations a column can have', () => {
  it('are the common ones for a text', () => {
    expect(calculationOptions(property('text'))).toEqual([
      'none',
      'count',
      'countEmpty',
      'countNotEmpty',
      'percentEmpty',
      'percentNotEmpty',
      'countValue',
      'countUniqueValue',
    ]);
  });

  it('add the arithmetic for a number', () => {
    expect(calculationOptions(property('number'))).toEqual(
      expect.arrayContaining(['sum', 'average', 'median', 'min', 'max', 'range']),
    );
  });

  it('add the ticks for a checkbox and the ends for a date', () => {
    expect(calculationOptions(property('checkbox')).slice(-4)).toEqual([
      'countChecked',
      'countUnchecked',
      'percentChecked',
      'percentUnchecked',
    ]);
    expect(calculationOptions(property('date')).slice(-3)).toEqual([
      'earliest',
      'latest',
      'dateRange',
    ]);
    expect(calculationOptions(property('createdTime'))).toContain('earliest');
  });

  it('never list a calculation twice', () => {
    for (const type of ['text', 'number', 'checkbox', 'date', 'createdTime', 'select'] as const) {
      const options = calculationOptions(property(type));
      expect(new Set(options).size, type).toBe(options.length);
    }
  });
});

describe('the calculation of a column', () => {
  it('is what the view stores', () => {
    expect(calculationOf({ p: 'sum' }, property('number'))).toBe('sum');
  });

  it('is the count for the title column and none for the others when nothing is stored', () => {
    expect(calculationOf({}, titleProperty('Name'))).toBe('count');
    expect(calculationOf({}, property('number'))).toBe('none');
  });

  it('is none for a stored name that is unknown or does not fit the kind', () => {
    expect(calculationOf({ p: 'sum' }, property('text'))).toBe('none');
    expect(calculationOf({ p: 'whatever' }, property('number'))).toBe('none');
  });

  it('can be turned off for the title column', () => {
    expect(calculationOf({ [TITLE_COLUMN_ID]: 'none' }, titleProperty('Name'))).toBe('none');
  });
});

describe('the names', () => {
  it('exist for every calculation, long and short', () => {
    for (const id of ['none', ...CALCULATION_IDS] as const) {
      expect(calculationLabel(id), id).not.toBe('');
      expect(calculationShortName(id), id).not.toBe('');
    }
  });

  it('are the names people know them by', () => {
    expect(calculationLabel('countUniqueValue')).toBe('Count unique values');
    expect(calculationShortName('countUniqueValue')).toBe('Unique');
    expect(calculationShortName('none')).toBe('Calculate');
  });

  it('are written out for the counts of empty and not empty', () => {
    expect(calculationLabel('countEmpty')).toBe('Count empty');
    expect(calculationShortName('percentNotEmpty')).toBe('Not empty');
  });
});

describe('the result', () => {
  const cards = [card('3'), card('4'), card()];

  it('is empty without a calculation', () => {
    expect(calculationResult('none', cards, property('number'), CONTEXT)).toBe('');
  });

  it('is worked out over the cards given', () => {
    expect(calculationResult('sum', cards, property('number'), CONTEXT)).toBe('7');
    expect(calculationResult('countEmpty', cards, property('number'), CONTEXT)).toBe('1');
    expect(calculationResult('count', cards, titleProperty('Name'), CONTEXT)).toBe('3');
  });

  it('counts the ticks of a checkbox', () => {
    const ticked = [card('true'), card('true'), card()];
    expect(calculationResult('countChecked', ticked, property('checkbox'), CONTEXT)).toBe('2');
    expect(calculationResult('percentUnchecked', ticked, property('checkbox'), CONTEXT)).toBe(
      '33%',
    );
  });
});
