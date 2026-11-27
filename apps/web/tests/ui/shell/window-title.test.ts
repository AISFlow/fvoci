/**
 * The window's title (`layout/window-title.svelte.ts`): what is shown before
 * what holds it, the app's name after, and the app's name alone for nothing.
 */
import { describe, expect, it } from 'vitest';

import { windowTitle } from '$lib/layout/window-title.svelte.ts';

describe('the window title', () => {
  it('names the item, then what holds it, then the app', () => {
    expect(windowTitle(['Book the venue', 'Launch plan'])).toBe(
      'Book the venue · Launch plan — fvoci',
    );
    expect(windowTitle(['일기', '11월 26일'])).toBe('일기 · 11월 26일 — fvoci');
  });

  it('leaves out what is not there, and is the app alone for nothing', () => {
    expect(windowTitle([undefined, 'Launch plan'])).toBe('Launch plan — fvoci');
    expect(windowTitle([' ', undefined])).toBe('fvoci');
  });
});
