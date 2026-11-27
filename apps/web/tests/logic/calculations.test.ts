/**
 * The calculations of a column. Four cases (marked) pin that a checkbox
 * holding the text `false` is not ticked: Q9 in docs/notes.md.
 */
import { describe, expect, it, test } from 'vitest';

import { CALCULATIONS } from '$lib/logic/calculations.ts';
import { setCardProperty, type PropertyTemplate } from '$lib/logic/wire.ts';

import { createTestBoard, createTestCard } from './support/factory.ts';

describe('the calculations of a column', () => {
  const board = createTestBoard();

  const card1 = createTestCard(board);
  setCardProperty(card1, 'property_text', 'lorem ipsum');
  setCardProperty(card1, 'property_number', '100');
  setCardProperty(card1, 'property_email', 'foobar@example.com');
  setCardProperty(card1, 'property_phone', '+1 1234567890');
  setCardProperty(card1, 'property_url', 'example.com');
  setCardProperty(card1, 'property_select', 'option_id_1');
  setCardProperty(card1, 'property_multiSelect', ['option_id_1', 'option_id_2', 'option_id_3']);
  setCardProperty(card1, 'property_date', '1625553000000');
  setCardProperty(card1, 'property_person', 'user_id_1');
  setCardProperty(card1, 'property_checkbox', 'true');
  card1.createdBy = 'user_id_1';
  card1.createdAt = 1625553000000;
  card1.modifiedBy = 'user_id_1';
  card1.updatedAt = 1625553000000;

  const card2 = createTestCard(board);
  setCardProperty(card2, 'property_text', 'foo bar');
  setCardProperty(card2, 'property_number', '-30');
  setCardProperty(card2, 'property_email', 'loremipsum@example.com');
  setCardProperty(card2, 'property_phone', '+1 111');
  setCardProperty(card2, 'property_url', 'example.com/foobar');
  setCardProperty(card2, 'property_select', 'option_id_2');
  setCardProperty(card2, 'property_multiSelect', ['option_id_2', 'option_id_3']);
  setCardProperty(card2, 'property_date', '1625639400000');
  setCardProperty(card2, 'property_person', 'user_id_2');
  setCardProperty(card2, 'property_checkbox', 'false');
  card2.createdAt = 1625639400000;
  card2.createdBy = 'user_id_2';
  card2.updatedAt = 1625639400000;
  card2.modifiedBy = 'user_id_2';

  // card with all properties unset
  const card3 = createTestCard(board);
  card3.createdAt = 1625639400000;
  card3.createdBy = 'user_id_2';
  card3.updatedAt = 1625639400000;
  card3.modifiedBy = 'user_id_2';

  // clone of card 1. All properties exactly same as that of card 1
  const card4 = createTestCard(board);
  setCardProperty(card4, 'property_text', 'lorem ipsum');
  setCardProperty(card4, 'property_number', '100');
  setCardProperty(card4, 'property_email', 'foobar@example.com');
  setCardProperty(card4, 'property_phone', '+1 1234567890');
  setCardProperty(card4, 'property_url', 'example.com');
  setCardProperty(card4, 'property_select', 'option_id_1');
  setCardProperty(card4, 'property_multiSelect', ['option_id_1', 'option_id_2', 'option_id_3']);
  setCardProperty(card4, 'property_date', '1625553000000');
  setCardProperty(card4, 'property_person', 'user_id_1');
  setCardProperty(card4, 'property_checkbox', 'true');
  card4.createdAt = 1625553000000;
  card4.createdBy = 'user_id_1';
  card4.updatedAt = 1625553000000;
  card4.modifiedBy = 'user_id_1';

  // card with all empty values
  const card5 = createTestCard(board);
  setCardProperty(card5, 'property_text', '');
  setCardProperty(card5, 'property_number', '');
  setCardProperty(card5, 'property_email', '');
  setCardProperty(card5, 'property_phone', '');
  setCardProperty(card5, 'property_url', '');
  setCardProperty(card5, 'property_select', '');
  setCardProperty(card5, 'property_multiSelect', []);
  setCardProperty(card5, 'property_date', '');
  setCardProperty(card5, 'property_person', '');
  setCardProperty(card5, 'property_checkbox', '');

  // clone of card 3 but created / updated 1 second later
  const card6 = createTestCard(board);
  card6.createdAt = 1625639401000;
  card6.createdBy = 'user_id_2';
  card6.updatedAt = 1625639401000;
  card6.modifiedBy = 'user_id_2';

  // clone of card 3 but created / updated 1 minute later
  const card7 = createTestCard(board);
  card7.createdAt = 1625639460000;
  card7.createdBy = 'user_id_2';
  card7.updatedAt = 1625639460000;
  card7.modifiedBy = 'user_id_2';

  const cards = [card1, card2, card3, card4];

  const templates = {
    text: { id: 'property_text', type: 'text', name: '', options: [] },
    number: { id: 'property_number', type: 'number', name: '', options: [] },
    email: { id: 'property_email', type: 'email', name: '', options: [] },
    phone: { id: 'property_phone', type: 'phone', name: '', options: [] },
    url: { id: 'property_url', type: 'url', name: '', options: [] },
    select: {
      id: 'property_select',
      type: 'select',
      name: '',
      options: [
        {
          color: 'yellow',
          id: 'option_id_1',
          label: 'Option 1',
        },
        {
          color: 'blue',
          id: 'option_id_2',
          label: 'Option 2',
        },
      ],
    },
    multiSelect: {
      id: 'property_multiSelect',
      type: 'multiSelect',
      name: '',
      options: [
        {
          color: 'yellow',
          id: 'option_id_1',
          label: 'Option 1',
        },
        {
          color: 'blue',
          id: 'option_id_2',
          label: 'Option 2',
        },
        {
          color: 'blue',
          id: 'option_id_3',
          label: 'Option 3',
        },
      ],
    },
    date: { id: 'property_date', type: 'date', name: '', options: [] },
    person: { id: 'property_person', type: 'person', name: '', options: [] },
    checkbox: { id: 'property_checkbox', type: 'checkbox', name: '', options: [] },
    createdTime: { id: 'property_createdTime', type: 'createdTime', name: '', options: [] },
    createdBy: { id: 'property_createdBy', type: 'createdBy', name: '', options: [] },
    updatedTime: { id: 'property_lastUpdatedTime', type: 'updatedTime', name: '', options: [] },
    updatedBy: { id: 'property_lastUpdatedBy', type: 'updatedBy', name: '', options: [] },
  } satisfies Record<string, PropertyTemplate>;
  type PropertyName = keyof typeof templates;
  const properties: Record<PropertyName, PropertyTemplate> = templates;

  const autofilledProperties = new Set([
    properties.createdBy,
    properties.createdTime,
    properties.updatedBy,
    properties.updatedTime,
  ]);

  const intl = { locale: 'en-us', timeZone: 'Asia/Seoul' };

  // testing count
  Object.values(properties).forEach((property) => {
    it(`should correctly count for property type "${property.type}"`, () => {
      expect(CALCULATIONS.count(cards, property, intl)).toBe('4');
    });
  });

  // testing count empty
  Object.values(properties)
    .filter((p) => !autofilledProperties.has(p))
    .forEach((property) => {
      it(`should correctly count empty for property type "${property.type}"`, () => {
        expect(CALCULATIONS.countEmpty(cards, property, intl)).toBe('1');
      });
    });

  // testing percent empty
  Object.values(properties)
    .filter((p) => !autofilledProperties.has(p))
    .forEach((property) => {
      it(`should correctly compute empty percent for property type "${property.type}"`, () => {
        expect(CALCULATIONS.percentEmpty(cards, property, intl)).toBe('25%');
      });
    });

  // testing count not empty
  Object.values(properties)
    .filter((p) => !autofilledProperties.has(p))
    .forEach((property) => {
      it(`should correctly count not empty for property type "${property.type}"`, () => {
        expect(CALCULATIONS.countNotEmpty(cards, property, intl)).toBe('3');
      });
    });

  // testing percent not empty
  Object.values(properties)
    .filter((p) => !autofilledProperties.has(p))
    .forEach((property) => {
      it(`should correctly compute not empty percent for property type "${property.type}"`, () => {
        expect(CALCULATIONS.percentNotEmpty(cards, property, intl)).toBe('75%');
      });
    });

  // testing countValues
  const countValueTests: Record<PropertyName, string> = {
    text: '3',
    number: '3',
    email: '3',
    phone: '3',
    url: '3',
    select: '3',
    multiSelect: '8',
    date: '3',
    person: '3',
    checkbox: '3',
    createdTime: '4',
    createdBy: '4',
    updatedTime: '4',
    updatedBy: '4',
  };
  (Object.keys(countValueTests) as PropertyName[]).forEach((propertyType) => {
    it(`should correctly count values for property type ${propertyType}`, () => {
      expect(CALCULATIONS.countValue(cards, properties[propertyType], intl)).toBe(
        countValueTests[propertyType],
      );
    });
  });

  // testing countUniqueValue
  const countUniqueValueTests: Record<PropertyName, string> = {
    text: '2',
    number: '2',
    email: '2',
    phone: '2',
    url: '2',
    select: '2',
    multiSelect: '3',
    date: '2',
    person: '2',
    checkbox: '2',
    createdTime: '2',
    createdBy: '2',
    updatedTime: '2',
    updatedBy: '2',
  };
  (Object.keys(countUniqueValueTests) as PropertyName[]).forEach((propertyType) => {
    it(`should correctly count unique values for property type ${propertyType}`, () => {
      expect(CALCULATIONS.countUniqueValue(cards, properties[propertyType], intl)).toBe(
        countUniqueValueTests[propertyType],
      );
    });
  });

  test('countUniqueValue for cards created 1 second apart', () => {
    const result = CALCULATIONS.countUniqueValue([card3, card6], properties.createdTime, intl);
    expect(result).toBe('1');
  });

  test('countUniqueValue for cards updated 1 second apart', () => {
    const result = CALCULATIONS.countUniqueValue([card3, card6], properties.updatedTime, intl);
    expect(result).toBe('1');
  });

  test('countUniqueValue for cards created 1 minute apart', () => {
    const result = CALCULATIONS.countUniqueValue([card3, card7], properties.createdTime, intl);
    expect(result).toBe('2');
  });

  test('countUniqueValue for cards updated 1 minute apart', () => {
    const result = CALCULATIONS.countUniqueValue([card3, card7], properties.updatedTime, intl);
    expect(result).toBe('2');
  });

  // Q9: card2 holds `false`, which is not a tick.
  test('countChecked for cards', () => {
    const result = CALCULATIONS.countChecked(cards, properties.checkbox, intl);
    expect(result).toBe('2');
  });

  test('countChecked for cards, one set, other unset', () => {
    const result = CALCULATIONS.countChecked([card1, card5], properties.checkbox, intl);
    expect(result).toBe('1');
  });

  // Q9: card2 holds `false`, which is not a tick.
  test('countUnchecked for cards', () => {
    const result = CALCULATIONS.countUnchecked(cards, properties.checkbox, intl);
    expect(result).toBe('2');
  });

  test('countUnchecked for cards, two set, one unset', () => {
    const result = CALCULATIONS.countUnchecked([card1, card1, card5], properties.checkbox, intl);
    expect(result).toBe('1');
  });

  test('countUnchecked for cards, one set, other unset', () => {
    const result = CALCULATIONS.countUnchecked([card1, card5], properties.checkbox, intl);
    expect(result).toBe('1');
  });

  test('countUnchecked for cards, one set, two unset', () => {
    const result = CALCULATIONS.countUnchecked([card1, card5, card5], properties.checkbox, intl);
    expect(result).toBe('2');
  });

  // Q9: card2 holds `false`, which is not a tick.
  test('percentChecked for cards', () => {
    const result = CALCULATIONS.percentChecked(cards, properties.checkbox, intl);
    expect(result).toBe('50%');
  });

  // Q9: card2 holds `false`, which is not a tick.
  test('percentUnchecked for cards', () => {
    const result = CALCULATIONS.percentUnchecked(cards, properties.checkbox, intl);
    expect(result).toBe('50%');
  });

  test('sum', () => {
    const result = CALCULATIONS.sum(cards, properties.number, intl);
    expect(result).toBe('170');
  });

  test('average', () => {
    const result = CALCULATIONS.average(cards, properties.number, intl);
    expect(result).toBe('56.67');
  });

  test('median', () => {
    const result = CALCULATIONS.median(cards, properties.number, intl);
    expect(result).toBe('100');
  });

  test('min', () => {
    const result = CALCULATIONS.min(cards, properties.number, intl);
    expect(result).toBe('-30');
  });

  test('max', () => {
    const result = CALCULATIONS.max(cards, properties.number, intl);
    expect(result).toBe('100');
  });

  test('range', () => {
    const result = CALCULATIONS.range(cards, properties.number, intl);
    expect(result).toBe('-30 - 100');
  });
});
