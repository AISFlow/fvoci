/**
 * The numbers of cards as a reader writes and finds them: what text names a
 * number, how one is shown, what `#` and what was typed after it name, and
 * a view's search box finding a card by its number.
 */
import { describe, expect, it } from 'vitest';

import {
  cardNumberOf,
  cardNumberText,
  cardsNamed,
  hashCardNumberOf,
  NAMED_AT_MOST,
} from '$lib/logic/card-numbers.ts';
import { searchFilterCards } from '$lib/logic/search.ts';
import { createCard, type Card } from '$lib/logic/wire.ts';

function card(id: string, title: string, number: number | undefined, updatedAt = 1): Card {
  const made = createCard();
  return {
    ...made,
    id,
    title,
    updatedAt,
    ...(number === undefined ? {} : { number }),
  };
}

describe('the number a text names', () => {
  it('is the digits, with or without `#`, around spaces', () => {
    expect(cardNumberOf('12')).toBe(12);
    expect(cardNumberOf('#12')).toBe(12);
    expect(cardNumberOf('  #3 ')).toBe(3);
    expect(cardNumberOf('2147483647')).toBe(2_147_483_647);
  });

  it('is none for what the server takes as no number of a card', () => {
    for (const text of [
      '',
      '#',
      '0',
      '#0',
      '012',
      '#012',
      '+1',
      '-1',
      '1.5',
      '1e3',
      '# 1',
      '##1',
    ]) {
      expect(cardNumberOf(text), text).toBeUndefined();
    }
    expect(cardNumberOf('2147483648')).toBeUndefined();
    expect(cardNumberOf('12345678901')).toBeUndefined();
    expect(cardNumberOf('#12 plan')).toBeUndefined();
  });

  it('is shown with its sign', () => {
    expect(cardNumberText(7)).toBe('#7');
  });

  it('normalizes `#N` and `#00N` for workspace-wide card number search, rejecting bare digits and mixed text', () => {
    expect(hashCardNumberOf('#12')).toBe(12);
    expect(hashCardNumberOf('#0012')).toBe(12);
    expect(hashCardNumberOf('  #003 ')).toBe(3);
    expect(hashCardNumberOf('#2147483647')).toBe(2_147_483_647);
    for (const text of [
      '',
      '12',
      '#',
      '#0',
      '#000',
      '#-1',
      '#+1',
      '# 1',
      '##1',
      '#12abc',
      '#12 plan',
      '#2147483648',
    ]) {
      expect(hashCardNumberOf(text), text).toBeUndefined();
    }
  });
});

describe('what `#` and what follows it name', () => {
  const cards = [
    card('a', 'Plan the road', 1, 5),
    card('b', 'Road works', 12, 9),
    card('c', 'Paint 1 fence', 10, 7),
    card('d', 'Other', 2, 3),
    card('t', 'A template of 1', undefined, 8),
  ];
  const ids = (typed: string): string[] => cardsNamed(cards, typed).map((one) => one.id);

  it('for digits, the card of that number first, then the numbers they begin, then titles', () => {
    expect(ids('1')).toEqual(['a', 'c', 'b', 't']);
    expect(ids('12')).toEqual(['b']);
    // A space typed after the digits: still the card of that number first.
    expect(ids('12 ')).toEqual(['b']);
    expect(ids('1 ')).toEqual(['a', 'c', 'b', 't']);
    expect(ids('2')).toEqual(['d']);
    expect(ids('99')).toEqual([]);
  });

  it('for words, the cards whose title has every one, whatever the case, the one changed last first', () => {
    expect(ids('ROAD')).toEqual(['b', 'a']);
    expect(ids('road plan')).toEqual(['a']);
    expect(ids('road nowhere')).toEqual([]);
  });

  it('is nothing for nothing typed, and no more than a list holds', () => {
    expect(ids('')).toEqual([]);
    expect(ids('  ')).toEqual([]);
    const many = Array.from({ length: 20 }, (_, at) => card(`m${String(at)}`, 'Many', at + 1));
    expect(cardsNamed(many, 'many')).toHaveLength(NAMED_AT_MOST);
  });
});

describe('the search box of a view', () => {
  const cards = [card('a', 'Alpha', 3), card('b', 'Item 3 of the list', 7), card('c', 'Gamma', 30)];
  const found = (text: string): string[] => searchFilterCards(cards, [], text).map((one) => one.id);

  it('finds the card of a number, and the titles that have its digits', () => {
    expect(found('#3')).toEqual(['a']);
    expect(found('3')).toEqual(['a', 'b']);
    expect(found('30')).toEqual(['c']);
    expect(found('#7')).toEqual(['b']);
    expect(found('#9')).toEqual([]);
  });
});
