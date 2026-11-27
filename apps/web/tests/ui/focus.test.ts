/** A focus that is on nothing: the page, or a box that only holds it around where it would go. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { focusAdrift, focusRevealed } from '$lib/focus.ts';

function page(): { box: HTMLElement; home: HTMLElement; other: HTMLElement; aside: HTMLElement } {
  document.body.innerHTML =
    '<div id="box" tabindex="-1"><button id="home">No date</button><button id="other">Today</button></div>' +
    '<div id="aside" tabindex="-1"></div>';
  const find = (id: string): HTMLElement => {
    const element = document.getElementById(id);
    if (!element) throw new Error(`no #${id}`);
    return element;
  };
  return { box: find('box'), home: find('home'), other: find('other'), aside: find('aside') };
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('a focus that is adrift', () => {
  it('is one on the page itself', () => {
    const { home } = page();
    expect(document.activeElement).toBe(document.body);
    expect(focusAdrift(home)).toBe(true);
    expect(focusAdrift(null)).toBe(true);
  });

  it('is one on a box around its home that only holds it, as Safari leaves it after a press', () => {
    const { box, home } = page();
    box.focus();
    expect(focusAdrift(home)).toBe(true);
  });

  it('is not one on a control, on its home, or on a box elsewhere', () => {
    const { home, other, aside } = page();
    other.focus();
    expect(focusAdrift(home)).toBe(false);
    home.focus();
    expect(focusAdrift(home)).toBe(false);
    // A dialog that took the focus meanwhile keeps it.
    aside.focus();
    expect(focusAdrift(home)).toBe(false);
  });

  it('is not one on a box when nothing says where it would go', () => {
    const { box } = page();
    box.focus();
    expect(focusAdrift(undefined)).toBe(false);
  });
});

describe('focusing an element that was just revealed', () => {
  it('flushes layout before focusing a connected element, forwarding options', () => {
    const { home } = page();
    const order: string[] = [];
    vi.spyOn(home, 'getBoundingClientRect').mockImplementation(() => {
      order.push('layout');
      return new DOMRect();
    });
    const focus = vi.spyOn(home, 'focus').mockImplementation(() => {
      order.push('focus');
    });

    focusRevealed(home, { preventScroll: true });

    expect(order).toEqual(['layout', 'focus']);
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it('does nothing for a missing or detached element', () => {
    const detached = document.createElement('button');
    const focus = vi.spyOn(detached, 'focus');

    focusRevealed(null);
    focusRevealed(undefined);
    focusRevealed(detached);

    expect(focus).not.toHaveBeenCalled();
  });
});
