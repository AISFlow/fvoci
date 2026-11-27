import { describe, expect, it } from 'vitest';

import { trashDaysLeft } from '../../src/lib/logic/trash-days.ts';

const DAY = 24 * 60 * 60 * 1000;
const deletedAt = Date.UTC(2026, 10, 1, 12);

describe('trashDaysLeft', () => {
  it('counts every day kept for one deleted just now', () => {
    expect(trashDaysLeft(deletedAt, 30, deletedAt)).toBe(30);
  });

  it('counts a day begun as a whole one', () => {
    expect(trashDaysLeft(deletedAt, 30, deletedAt + 1)).toBe(30);
    expect(trashDaysLeft(deletedAt, 30, deletedAt + DAY)).toBe(29);
    expect(trashDaysLeft(deletedAt, 30, deletedAt + 29 * DAY + 1)).toBe(1);
  });

  it('is 0 once the time is up, and stays there', () => {
    expect(trashDaysLeft(deletedAt, 30, deletedAt + 30 * DAY)).toBe(0);
    expect(trashDaysLeft(deletedAt, 30, deletedAt + 45 * DAY)).toBe(0);
  });

  it('follows the days the server keeps', () => {
    expect(trashDaysLeft(deletedAt, 7, deletedAt + 2 * DAY)).toBe(5);
  });
});
