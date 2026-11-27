/** A part of a panel that folds (`Fold`): open by itself, folded or unfolded by hand. */
import { describe, expect, it } from 'vitest';

import { Fold } from '$lib/components/fold/fold.svelte.ts';

describe('a part that folds', () => {
  it('opens by itself once it holds something, and stays open when the last of it goes', () => {
    let held = 0;
    const fold = new Fold(() => held > 0);
    expect(fold.open).toBe(false);
    held = 1;
    expect(fold.open).toBe(true);
    held = 0;
    expect(fold.open).toBe(true);
  });

  it('holds what the reader chose, either way', () => {
    let held = 1;
    const fold = new Fold(() => held > 0);
    fold.toggle();
    expect(fold.open).toBe(false);
    held = 2;
    expect(fold.open).toBe(false);
    fold.toggle();
    held = 0;
    expect(fold.open).toBe(true);
  });

  it('stays folded by itself while it holds nothing', () => {
    const fold = new Fold();
    expect(fold.open).toBe(false);
    fold.toggle();
    expect(fold.open).toBe(true);
  });
});
