/**
 * A name shown cut short has its whole self as its tooltip, and one shown
 * whole has none (components/cut-title.ts). A simulated page lays nothing
 * out: the widths are set here.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { cutTitle } from '$lib/components/cut-title.ts';

function line(): { element: HTMLElement; widths: (scroll: number, client: number) => void } {
  const element = document.createElement('a');
  const text = document.createElement('span');
  text.className = 'truncate';
  element.append(text);
  document.body.append(element);
  const widths = (scroll: number, client: number): void => {
    Object.defineProperty(text, 'scrollWidth', { value: scroll, configurable: true });
    Object.defineProperty(text, 'clientWidth', { value: client, configurable: true });
  };
  return { element, widths };
}

const pointedAt = (element: HTMLElement): void => {
  element.dispatchEvent(new Event('pointerenter'));
};

afterEach(() => {
  document.body.replaceChildren();
});

describe('the tooltip of a name', () => {
  it('is the whole name while it is cut, as the pointer comes over it, and none once it fits', async () => {
    const { element, widths } = line();
    const stop = cutTitle('Everything we ship this quarter')(element);
    // Svelte listens for the pointer a moment after.
    await Promise.resolve();
    widths(300, 120);
    expect(element.hasAttribute('title')).toBe(false);
    pointedAt(element);
    expect(element.title).toBe('Everything we ship this quarter');
    // The pointer gone, the tooltip goes: a reader who comes by the keys is not read it.
    element.dispatchEvent(new Event('pointerleave'));
    expect(element.hasAttribute('title')).toBe(false);
    // Wider now (the window, the name): shown whole, no tooltip of the same words.
    widths(120, 120);
    pointedAt(element);
    expect(element.hasAttribute('title')).toBe(false);
    // Taken away with the label it was for (another name shown), the pointer still over it.
    widths(300, 120);
    pointedAt(element);
    expect(element.title).toBe('Everything we ship this quarter');
    if (typeof stop === 'function') stop();
    expect(element.hasAttribute('title')).toBe(false);
    pointedAt(element);
    expect(element.hasAttribute('title')).toBe(false);
  });
});
