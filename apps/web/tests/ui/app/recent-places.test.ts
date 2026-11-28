/**
 * What each reader opened last on this browser (`recent-places.ts`): the
 * latest first, and nothing that is no identifier, which would have the
 * server refuse what the search asks with it.
 */
import { beforeEach, describe, expect, it } from 'vitest';

import {
  forgetRecentPlaces,
  recentPlaces,
  rememberRecent,
} from '../../../src/app/recent-places.ts';

const A = '0193f0a2-0000-7000-8000-00000000000a';
const B = '0193f0a2-0000-7000-8000-00000000000b';

beforeEach(() => {
  localStorage.clear();
});

describe('the places opened last', () => {
  it('keeps the latest first, and nothing that is no identifier', () => {
    rememberRecent('me', 'w', { id: A, kind: 'container' });
    rememberRecent('me', 'w', { id: B, kind: 'card' });
    rememberRecent('me', 'w', { id: 'not-an-id', kind: 'card' });
    rememberRecent('me', 'w', { id: A.toUpperCase(), kind: 'card' });
    rememberRecent('me', 'w', { id: '00000000-0000-0000-0000-000000000000', kind: 'page' });
    expect(recentPlaces('me', 'w')).toEqual([
      { id: B, kind: 'card' },
      { id: A, kind: 'container' },
    ]);
  });

  it('reads back no place that is not one, whatever the storage holds', () => {
    localStorage.setItem(
      'fvoci:recent:me:w',
      JSON.stringify([
        { id: 'board:1', kind: 'container' },
        { id: A, kind: 'card' },
        { id: B, kind: 'diary' },
      ]),
    );
    expect(recentPlaces('me', 'w')).toEqual([{ id: A, kind: 'card' }]);
    forgetRecentPlaces();
    expect(recentPlaces('me', 'w')).toEqual([]);
  });
});
