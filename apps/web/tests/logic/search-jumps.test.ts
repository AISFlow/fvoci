import { describe, expect, it } from 'vitest';

import { JUMPS_SHOWN, searchJumps } from '../../src/lib/logic/search-jumps.ts';

const made = (title: string, more: { isTemplate?: boolean; deletedAt?: number } = {}) => ({
  title,
  isTemplate: more.isTemplate ?? false,
  deletedAt: more.deletedAt,
});
const titleOf = (container: { title: string }): string => container.title;

describe('searchJumps', () => {
  const containers = [
    made('Roadmap'),
    made('Road trip template', { isTemplate: true }),
    made('Handbook'),
    made('Old road', { deletedAt: 1 }),
    made('Reading list'),
  ];

  it('offers the first of them before anything is typed, without templates or deleted ones', () => {
    expect(searchJumps(containers, '  ', titleOf).map(titleOf)).toEqual([
      'Roadmap',
      'Handbook',
      'Reading list',
    ]);
  });

  it('offers those whose title holds what was typed, whatever its case', () => {
    expect(searchJumps(containers, 'ROAD', titleOf).map(titleOf)).toEqual(['Roadmap']);
    expect(searchJumps(containers, 'book', titleOf).map(titleOf)).toEqual(['Handbook']);
    expect(searchJumps(containers, 'nothing', titleOf)).toEqual([]);
  });

  it('offers a few at most', () => {
    const many = Array.from({ length: JUMPS_SHOWN + 3 }, (_, index) => made(`Board ${index}`));
    expect(searchJumps(many, 'board', titleOf)).toHaveLength(JUMPS_SHOWN);
  });
});
