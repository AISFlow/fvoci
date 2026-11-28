import { describe, expect, it } from 'vitest';

import { favoriteOrderAfter } from '../../src/lib/logic/favorite-order.ts';

describe('favoriteOrderAfter', () => {
  const ids = ['a', 'b', 'c'];

  it('puts the star at the place it was dropped at, counted without it', () => {
    expect(favoriteOrderAfter(ids, 'a', { kind: 'index', listId: 'favorites', index: 2 })).toEqual([
      'b',
      'c',
      'a',
    ]);
    expect(favoriteOrderAfter(ids, 'c', { kind: 'index', listId: 'favorites', index: 0 })).toEqual([
      'c',
      'a',
      'b',
    ]);
  });

  it('moves nothing where it was, past the end, or for what is no star', () => {
    expect(
      favoriteOrderAfter(ids, 'b', { kind: 'index', listId: 'favorites', index: 1 }),
    ).toBeUndefined();
    expect(favoriteOrderAfter(ids, 'c', { kind: 'index', listId: 'favorites', index: 9 })).toBe(
      undefined,
    );
    expect(favoriteOrderAfter(ids, 'z', { kind: 'index', listId: 'favorites', index: 0 })).toBe(
      undefined,
    );
    expect(favoriteOrderAfter(ids, 'a', { kind: 'list', listId: 'favorites' })).toBeUndefined();
  });
});
