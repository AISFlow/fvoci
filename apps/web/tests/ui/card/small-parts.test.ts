/**
 * The small parts of the card detail that are plain functions: the emoji a
 * card is given, the names and pictures of the kinds of property, the frame
 * classes, and the text box that fits its text where the browser does not
 * do that by itself.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { autosize, fitHeight } from '$lib/components/autosize.ts';
import { FRAME_CLASS } from '$lib/features/card/frame.ts';
import { CARD_EMOJI, propertyTypeIcon, randomEmoji } from '$lib/features/card/icons.ts';
import { propertyTypeName } from '$lib/features/card/property-names.ts';
import { cardFrame } from '$lib/layout/presentation.ts';
import { SELECTABLE_PROPERTY_TYPES } from '$lib/logic/properties.ts';

describe('the emoji a card is given', () => {
  it('is one of the list, for every random number', () => {
    expect(randomEmoji(0)).toBe(CARD_EMOJI[0]);
    expect(randomEmoji(0.999999)).toBe(CARD_EMOJI.at(-1));
    // A source of random numbers that misbehaves still gets an emoji.
    expect(CARD_EMOJI).toContain(randomEmoji(1));
    expect(CARD_EMOJI).toContain(randomEmoji(-0.5));
    expect(CARD_EMOJI).toContain(randomEmoji());
  });

  it('has no entry twice, and every entry is a single character to a reader', () => {
    expect(new Set(CARD_EMOJI).size).toBe(CARD_EMOJI.length);
    const segmenter = new Intl.Segmenter('en', { granularity: 'grapheme' });
    for (const emoji of CARD_EMOJI) {
      expect([...segmenter.segment(emoji)], emoji).toHaveLength(1);
    }
  });
});

describe('the kinds of property', () => {
  it('have a name and a picture each', () => {
    const names = SELECTABLE_PROPERTY_TYPES.map(propertyTypeName);
    expect(new Set(names).size).toBe(SELECTABLE_PROPERTY_TYPES.length);
    expect(names).not.toContain('Unknown');
    for (const type of SELECTABLE_PROPERTY_TYPES)
      expect(propertyTypeIcon(type)).toBeTypeOf('function');
  });

  it('are named in words, as people call them', () => {
    expect(propertyTypeName('multiSelect')).toBe('Multi select');
    expect(propertyTypeName('updatedBy')).toBe('Last updated by');
    expect(propertyTypeName('url')).toBe('URL');
  });

  it('fall back for a kind this app does not know', () => {
    expect(propertyTypeName('something new')).toBe('Unknown');
    expect(propertyTypeIcon('something new')).toBe(propertyTypeIcon('text'));
  });
});

describe('the classes of the three frames', () => {
  it('cover every frame a stage can ask for', () => {
    for (const stage of ['compact', 'medium', 'expanded'] as const) {
      expect(FRAME_CLASS[cardFrame(stage, 'center')]).toBeTruthy();
    }
  });

  it('set the same properties in every frame, so that one frame leaves nothing behind in another', () => {
    // What a class sets, by how its name begins. The longer beginnings come first.
    const SETS = [
      'sm:max-w-',
      'max-w-',
      '-translate-x-',
      'translate-x-',
      '-translate-y-',
      'translate-y-',
      'top-',
      'right-',
      'left-',
      'h-',
      'w-',
      'rounded-',
      'border-l',
      'bg-',
      // What a body in it is on (editor.css: the frame a table moves sideways in).
      '[--doc-surface:',
      'pt-',
      'pr-',
      'pl-',
      'shadow-',
      'ring-',
    ];
    const setBy = (name: string): string => {
      const found = SETS.find((beginning) => name.startsWith(beginning));
      if (!found) throw new Error(`a class of a frame that this test does not know: ${name}`);
      return found.replace(/^-/, '');
    };
    const every = [...new Set(SETS.map((beginning) => beginning.replace(/^-/, '')))].sort();
    for (const [frame, classes] of Object.entries(FRAME_CLASS)) {
      const set = classes.split(/\s+/).map(setBy);
      expect([...set].sort(), frame).toEqual(every);
    }
  });

  it('follow the on-screen keyboard in their height', () => {
    for (const classes of Object.values(FRAME_CLASS)) {
      expect(classes).toMatch(/h-\[[^\]]*var\(--keyboard\)/);
    }
  });
});

describe('a text box that fits its text', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function textarea(scrollHeight: number): HTMLTextAreaElement {
    const element = document.createElement('textarea');
    Object.defineProperty(element, 'scrollHeight', { configurable: true, get: () => scrollHeight });
    return element;
  }

  it('gets the height of its text', () => {
    const element = textarea(96);
    fitHeight(element);
    expect(element.style.height).toBe('96px');
  });

  it('is left to the browser where the browser sizes it', () => {
    vi.stubGlobal('CSS', { supports: () => true });
    const element = textarea(96);
    expect(autosize(() => 'text')(element)).toBeUndefined();
    expect(element.style.height).toBe('');
  });

  it('is fitted when made and on every input where the browser does not', () => {
    vi.stubGlobal('CSS', { supports: () => false });
    let height = 40;
    const element = document.createElement('textarea');
    Object.defineProperty(element, 'scrollHeight', { configurable: true, get: () => height });
    const stop = autosize(() => 'text')(element);
    expect(element.style.height).toBe('40px');
    height = 64;
    element.dispatchEvent(new Event('input'));
    expect(element.style.height).toBe('64px');
    stop?.();
    height = 88;
    element.dispatchEvent(new Event('input'));
    expect(element.style.height).toBe('64px');
  });

  it('reads the text it is given, so that a caller fits it again when the text changes', () => {
    vi.stubGlobal('CSS', { supports: () => false });
    const text = vi.fn(() => 'text');
    autosize(text)(textarea(10));
    expect(text).toHaveBeenCalledOnce();
  });
});
