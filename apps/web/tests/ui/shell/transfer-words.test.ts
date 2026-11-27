/** The words of the one card of many uploads (features/shell/transfer-words.ts). */
import { describe, expect, it } from 'vitest';

import { uploadsLabel } from '$lib/features/shell/transfer-words.ts';

describe('the card of many uploads', () => {
  it('counts what is going up, one file or several', () => {
    expect(uploadsLabel(5, 0, 5)).toBe('Uploading 5 files');
    // The others are in, and one is left: still one card, saying one.
    expect(uploadsLabel(1, 0, 5)).toBe('Uploading 1 file');
  });

  it('counts what failed once nothing goes up, then what is up', () => {
    expect(uploadsLabel(0, 2, 5)).toBe('2 not uploaded');
    expect(uploadsLabel(0, 0, 5)).toBe('5 files are uploaded.');
  });
});
