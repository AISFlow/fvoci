/** The hub of today's ask to write today (`src/app/write-today.ts`): taken once, by the next diary page, soon. */
import { describe, expect, it } from 'vitest';

import { ASK_WAITS, askToWriteToday, takeWriteToday } from '../../../src/app/write-today.ts';

describe('the ask to write today', () => {
  it('is taken once by the diary it names', () => {
    askToWriteToday('d1', 0);
    expect(takeWriteToday('d1', 10)).toBe(true);
    expect(takeWriteToday('d1', 20)).toBe(false);
  });

  it('is taken by another diary opened first, and so put nowhere later', () => {
    askToWriteToday('d1', 0);
    expect(takeWriteToday('d2', 10)).toBe(false);
    expect(takeWriteToday('d1', 20)).toBe(false);
  });

  it('is gone once it has waited too long', () => {
    askToWriteToday('d1', 0);
    expect(takeWriteToday('d1', ASK_WAITS)).toBe(false);
  });
});
