/**
 * The patches that take changes back, made right when a write under way
 * fails (state/inverses.ts): what a later patch takes back to that the
 * failed write showed becomes what it was to replace.
 */
import { describe, expect, it } from 'vitest';

import type { BlockPatch } from '$lib/logic/wire.ts';
import { Inverses } from '$lib/state/inverses.ts';

describe('a write that fails', () => {
  it('makes a patch kept while it was under way take back to what it replaced', async () => {
    const inverses = new Inverses<BlockPatch>('updatedFields', 'deletedFields');
    let fail = (): void => undefined;
    const failing = inverses
      .write(
        'v',
        { title: 'Shown', updatedFields: { sort: 'b' }, deletedFields: ['zoom'] },
        { title: 'Stored', updatedFields: { sort: 'a', zoom: 'week' } },
        () =>
          new Promise((_resolve, reject) => {
            fail = () => {
              reject(new Error('refused'));
            };
          }),
      )
      .catch(() => undefined);
    // A later change of the same view, made from what the failed write showed.
    const later: BlockPatch = {
      title: 'Shown',
      updatedFields: { sort: 'b', other: 1 },
      deletedFields: ['zoom'],
    };
    inverses.keep('v', later);
    fail();
    await failing;
    expect(later).toEqual({
      title: 'Stored',
      updatedFields: { other: 1, sort: 'a', zoom: 'week' },
      deletedFields: [],
    });
  });

  it('leaves alone a patch of another thing, one made before it, and one that says something else', async () => {
    const inverses = new Inverses<BlockPatch>('updatedFields', 'deletedFields');
    const before: BlockPatch = { title: 'Shown' };
    inverses.keep('v', before); // no write under way: nothing can fail before it
    let fail = (): void => undefined;
    const failing = inverses
      .write(
        'v',
        { title: 'Shown' },
        { title: 'Stored' },
        () =>
          new Promise((_resolve, reject) => {
            fail = () => {
              reject(new Error('refused'));
            };
          }),
      )
      .catch(() => undefined);
    const other: BlockPatch = { title: 'Shown' };
    const different: BlockPatch = { title: 'Else' };
    inverses.keep('w', other);
    inverses.keep('v', different);
    fail();
    await failing;
    expect([before.title, other.title, different.title]).toEqual(['Shown', 'Shown', 'Else']);
  });

  it('does not take an empty list of a board for one that says something', async () => {
    const inverses = new Inverses<Record<string, unknown>>(
      'updatedProperties',
      'deletedProperties',
    );
    const property = { id: 'p', name: 'Gone', type: 'text', options: [] };
    let fail = (): void => undefined;
    const failing = inverses
      .write(
        'b',
        { updatedCardProperties: [], deletedCardProperties: ['p'] },
        { updatedCardProperties: [property], deletedCardProperties: [] },
        () =>
          new Promise((_resolve, reject) => {
            fail = () => {
              reject(new Error('refused'));
            };
          }),
      )
      .catch(() => undefined);
    // The undo of a change of the title, made while the deletion was written.
    const later = { title: 'Before', updatedCardProperties: [], deletedCardProperties: [] };
    inverses.keep('b', later);
    fail();
    await failing;
    expect(later).toEqual({
      title: 'Before',
      updatedCardProperties: [],
      deletedCardProperties: [],
    });
  });
});
