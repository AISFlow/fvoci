/** The selection of cards in a view of a board (features/views/selection.svelte.ts). */
import { SvelteSet } from 'svelte/reactivity';
import { describe, expect, it } from 'vitest';

import { Selection, toggled, withRange } from '$lib/features/views/selection.svelte.ts';
import { CARD_BATCH_LIMIT } from '$lib/state/index.ts';

const ORDER = ['a', 'b', 'c', 'd'];

describe('the selection of a board', () => {
  it('starts empty, and a plain press does not select', () => {
    const selection = new Selection(() => ORDER);
    expect(selection.ids).toEqual([]);
    expect(selection.selecting).toBe(false);
  });

  it('takes cards in and out one by one, without changing what a plain press does', () => {
    const selection = new Selection(() => ORDER);
    selection.toggle('b');
    selection.toggle('d');
    expect(selection.ids).toEqual(['b', 'd']);
    expect(selection.selecting).toBe(false);
    selection.toggle('b');
    expect(selection.ids).toEqual(['d']);
  });

  it('takes a range from the card pressed last', () => {
    const selection = new Selection(() => ORDER);
    selection.toggle('a');
    selection.extendTo('c');
    expect(selection.ids).toEqual(['a', 'b', 'c']);
    // The range is measured from the card pressed last, also when that press took it out.
    selection.toggle('b');
    selection.extendTo('d');
    expect(selection.ids).toEqual(['a', 'c', 'b', 'd']);
  });

  it('from the menu of a card makes a plain press select, until nothing is selected', () => {
    const selection = new Selection(() => ORDER);
    selection.begin('a');
    expect(selection.selecting).toBe(true);
    expect(selection.ids).toEqual(['a']);
    selection.toggle('b');
    selection.toggle('a');
    expect(selection.selecting).toBe(true);
    selection.toggle('b');
    expect(selection.ids).toEqual([]);
    expect(selection.selecting).toBe(false);
  });

  it('is let go at once', () => {
    const selection = new Selection(() => ORDER);
    selection.begin('a');
    selection.toggle('c');
    selection.clear();
    expect(selection.ids).toEqual([]);
    expect(selection.selecting).toBe(false);
    // Nothing is left to measure a range from.
    selection.extendTo('c');
    expect(selection.ids).toEqual(['c']);
  });

  it('does not count a card the board no longer shows', () => {
    let shown = ORDER;
    const selection = new Selection(() => shown);
    selection.toggle('a');
    selection.toggle('c');
    shown = ['a', 'b', 'd'];
    // Read anew: outside a component nothing tells the selection that the board changed.
    selection.toggle('d');
    expect(selection.ids).toEqual(['a', 'd']);
  });
});

describe('selecting', () => {
  it('takes a card in, and out when it was in', () => {
    expect(toggled([], 'a')).toEqual(['a']);
    expect(toggled(['a', 'b'], 'a')).toEqual(['b']);
  });

  const order = ['a', 'b', 'c', 'd', 'e'];

  it('takes in the range from the anchor, in either direction', () => {
    expect(withRange(order, ['b'], 'b', 'd')).toEqual(['b', 'c', 'd']);
    expect(withRange(order, ['d'], 'd', 'b')).toEqual(['d', 'b', 'c']);
  });

  it('keeps what was selected outside the range', () => {
    expect(withRange(order, ['a', 'e'], 'e', 'c')).toEqual(['a', 'e', 'c', 'd']);
  });

  it('adds the card alone without an anchor on the board', () => {
    expect(withRange(order, [], undefined, 'c')).toEqual(['c']);
    expect(withRange(order, ['a'], 'gone', 'c')).toEqual(['a', 'c']);
  });

  it('changes nothing for a card that is not on the board', () => {
    expect(withRange(order, ['a'], 'a', 'gone')).toEqual(['a']);
  });
});

describe('the limit of a selection', () => {
  const many = Array.from({ length: CARD_BATCH_LIMIT + 10 }, (_, at) => `card-${at}`);

  it('takes in a range as far as the limit, and says it is full', () => {
    const selection = new Selection(() => many);
    selection.toggle('card-0');
    selection.extendTo(`card-${CARD_BATCH_LIMIT + 5}`);
    expect(selection.ids).toHaveLength(CARD_BATCH_LIMIT);
    expect(selection.ids.at(-1)).toBe(`card-${CARD_BATCH_LIMIT - 1}`);
    expect(selection.full).toBe(true);
  });

  it('keeps the cards nearest the one pressed first when a range upwards is too long', () => {
    const selection = new Selection(() => many);
    const last = CARD_BATCH_LIMIT + 5;
    selection.toggle(`card-${last}`);
    selection.extendTo('card-0');
    expect(selection.ids).toHaveLength(CARD_BATCH_LIMIT);
    // From the card pressed first upwards: card-6 to card-205; card-0 to card-5 are left out.
    const nearest = last - CARD_BATCH_LIMIT + 1;
    expect([...selection.ids].sort()).toEqual(many.slice(nearest, last + 1).sort());
    expect(selection.has(`card-${nearest - 1}`)).toBe(false);
    expect(selection.full).toBe(true);
  });

  it('takes no card in past the limit, and lets one out', () => {
    const selection = new Selection(() => many);
    selection.toggle('card-0');
    selection.extendTo(`card-${CARD_BATCH_LIMIT - 1}`);
    expect(selection.full).toBe(false);
    selection.toggle(`card-${CARD_BATCH_LIMIT}`);
    expect(selection.ids).toHaveLength(CARD_BATCH_LIMIT);
    expect(selection.has(`card-${CARD_BATCH_LIMIT}`)).toBe(false);
    expect(selection.full).toBe(true);
    selection.toggle('card-3');
    expect(selection.ids).toHaveLength(CARD_BATCH_LIMIT - 1);
    expect(selection.full).toBe(false);
  });
});

describe('Escape in a view', () => {
  it('lets the selection go, unless something else took the key', () => {
    const selection = new Selection(() => ['a', 'b']);
    const root = document.createElement('div');
    const detach = selection.attachEscape(root);
    selection.toggle('a');
    const taken = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    taken.preventDefault();
    root.dispatchEvent(taken);
    expect(selection.ids).toEqual(['a']);
    root.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(selection.ids).toEqual([]);
    detach();
  });
});

describe('"Select" without a selected card in the view', () => {
  it('ends once the view no longer shows a selected card, and a plain press opens again', () => {
    // Reactive, as the view's rows are: the selection reads the change.
    const shown = new SvelteSet(['a', 'b', 'c']);
    const selection = new Selection(() => [...shown]);
    selection.begin('a');
    expect(selection.selecting).toBe(true);
    shown.delete('a');
    expect(selection.ids).toEqual([]);
    expect(selection.selecting).toBe(false);
    expect(selection.press('b', 'open')).toBe(false);
    // The card comes back into the view: a plain press is the card's again until "Select".
    shown.add('a');
    selection.clear();
    expect(selection.press('b', 'open')).toBe(false);
  });

  it('goes on while a selected card is still in the view', () => {
    const shown = new SvelteSet(['a', 'b', 'c']);
    const selection = new Selection(() => [...shown]);
    selection.begin('a');
    selection.toggle('b');
    shown.delete('a');
    expect(selection.selecting).toBe(true);
    expect(selection.press('c', 'open')).toBe(true);
    expect(selection.ids).toEqual(['b', 'c']);
  });
});
