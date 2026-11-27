import { afterEach, describe, expect, it, vi } from 'vitest';

import { arrowChoice } from '$lib/components/arrow-choice.ts';

interface Group {
  root: HTMLElement;
  a: HTMLButtonElement;
  b: HTMLButtonElement;
  c: HTMLButtonElement;
  /** The Tab stops of the three, in order. */
  stops: () => number[];
}

/** A group of three answers made by hand, the second chosen unless told otherwise; a click chooses. */
function group(role: 'radio' | 'tab', chosen: 'a' | 'b' | 'c' | 'none' = 'b'): Group {
  const root = document.createElement('div');
  root.setAttribute('role', role === 'radio' ? 'radiogroup' : 'tablist');
  const mark = role === 'radio' ? 'aria-checked' : 'aria-selected';
  const all: HTMLButtonElement[] = [];
  const answer = (name: string): HTMLButtonElement => {
    const made = document.createElement('button');
    made.setAttribute('role', role);
    made.setAttribute(mark, String(name === chosen));
    made.textContent = name;
    made.addEventListener('click', () => {
      for (const each of all) each.setAttribute(mark, String(each === made));
    });
    all.push(made);
    root.append(made);
    return made;
  };
  const made = { root, a: answer('a'), b: answer('b'), c: answer('c') };
  document.body.append(root);
  return { ...made, stops: () => all.map((each) => each.tabIndex) };
}

function press(target: HTMLElement, key: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

const detachers: (() => void)[] = [];
function attach(root: HTMLElement, options?: Parameters<typeof arrowChoice>[0]): void {
  const detach = arrowChoice(options)(root);
  if (detach) detachers.push(detach);
}

afterEach(() => {
  for (const detach of detachers.splice(0)) detach();
  document.body.replaceChildren();
});

describe('the keys of a group of which one is chosen', () => {
  it('gives Tab one stop, at the chosen answer, or at the first where none is', () => {
    const one = group('radio');
    attach(one.root);
    expect(one.stops()).toEqual([-1, 0, -1]);

    const none = group('radio', 'none');
    attach(none.root);
    expect(none.stops()).toEqual([0, -1, -1]);
  });

  it('goes to the neighbour with an arrow, around the ends, and chooses it', () => {
    const { root, a, b, c, stops } = group('radio');
    attach(root);
    b.focus();
    const event = press(b, 'ArrowRight');
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(c);
    expect(c).toHaveAttribute('aria-checked', 'true');
    expect(stops()).toEqual([-1, -1, 0]);

    press(c, 'ArrowDown');
    expect(document.activeElement).toBe(a);
    expect(a).toHaveAttribute('aria-checked', 'true');

    press(a, 'ArrowLeft');
    expect(document.activeElement).toBe(c);
    press(c, 'ArrowUp');
    expect(document.activeElement).toBe(b);
  });

  it('goes to the ends with Home and End, in a list of tabs too', () => {
    const { root, a, b, c } = group('tab');
    attach(root);
    b.focus();
    press(b, 'End');
    expect(document.activeElement).toBe(c);
    expect(c).toHaveAttribute('aria-selected', 'true');
    press(c, 'Home');
    expect(document.activeElement).toBe(a);
    expect(a).toHaveAttribute('aria-selected', 'true');
  });

  it('only moves where choosing would close the surface', () => {
    const { root, b, c, stops } = group('radio');
    const clicked = vi.fn();
    c.addEventListener('click', clicked);
    attach(root, { choose: false });
    b.focus();
    press(b, 'ArrowDown');
    expect(document.activeElement).toBe(c);
    expect(clicked).not.toHaveBeenCalled();
    expect(b).toHaveAttribute('aria-checked', 'true');
    // Tab leaves from where the reader is, not from the chosen answer.
    expect(stops()).toEqual([-1, -1, 0]);
  });

  it('passes an answer that cannot be chosen, and leaves other keys and modified arrows alone', () => {
    const { root, a, b, c } = group('radio', 'a');
    b.disabled = true;
    attach(root);
    a.focus();
    expect(press(a, 'Enter').defaultPrevented).toBe(false);
    expect(press(a, 'ArrowRight', { altKey: true }).defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(a);

    press(a, 'ArrowRight');
    expect(document.activeElement).toBe(c);
  });

  it('follows a choice made from outside', async () => {
    const { root, a, stops } = group('radio');
    attach(root);
    a.click();
    await Promise.resolve();
    expect(stops()).toEqual([0, -1, -1]);
  });
});
