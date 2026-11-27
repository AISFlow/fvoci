/** The selection of cards on a board (features/views/kanban/selection.svelte.ts). */
import { describe, expect, it } from 'vitest';

import { Selection } from '$lib/features/views/kanban/selection.svelte.ts';

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
