/**
 * A finger's press that opens something from a body (touch-press.ts): kept
 * from the browser and the editor while the body is not being written, left
 * as it is while it is, and for a mouse. A long press on a comment
 * (`longPress`), but not on a link or a control inside it, and that keeps
 * the click of no press after it.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { keepTouchFromText, LONG_PRESS_MS, longPress } from '$lib/editor/touch-press.ts';

/** A body with a control in a part of it that is not text, as a block or a chip is drawn. */
function bodyWithControl(): { text: HTMLElement; control: HTMLElement } {
  const text = document.createElement('div');
  text.setAttribute('contenteditable', 'true');
  text.tabIndex = 0;
  const holder = document.createElement('div');
  holder.setAttribute('contenteditable', 'false');
  const control = document.createElement('button');
  holder.append(control);
  text.append(holder);
  document.body.append(text);
  keepTouchFromText(control);
  return { text, control };
}

/** Presses `control` with a pointer of `kind`: whether the browser may do what it does with the press. */
function press(control: HTMLElement, kind: string): boolean {
  const down = new Event('pointerdown', { bubbles: true });
  Object.defineProperty(down, 'pointerType', { value: kind });
  control.dispatchEvent(down);
  return control.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
}

afterEach(() => {
  document.body.replaceChildren();
});

describe('a press on a control in a body', () => {
  it('is kept from the browser and the editor when a finger presses in a body not being written', () => {
    const { control } = bodyWithControl();
    expect(press(control, 'touch')).toBe(false);
  });

  it('is left alone when the body is being written', () => {
    const { text, control } = bodyWithControl();
    text.focus();
    expect(document.activeElement).toBe(text);
    expect(press(control, 'touch')).toBe(true);
  });

  it('is left alone for a mouse and a pen', () => {
    const { control } = bodyWithControl();
    expect(press(control, 'mouse')).toBe(true);
    expect(press(control, 'pen')).toBe(true);
  });

  it('is left alone where a body is only shown', () => {
    const control = document.createElement('button');
    document.body.append(control);
    keepTouchFromText(control);
    expect(press(control, 'touch')).toBe(true);
  });
});

describe('a long press of a finger', () => {
  /** A comment with a word and a link in it, and how often it was pressed long. */
  function comment(): {
    word: HTMLElement;
    link: HTMLElement;
    button: HTMLElement;
    count: () => number;
  } {
    const element = document.createElement('li');
    const word = document.createElement('span');
    const link = document.createElement('a');
    link.href = 'https://example.com/';
    const button = document.createElement('button');
    element.append(word, link, button);
    document.body.append(element);
    let pressed = 0;
    longPress(() => (pressed += 1))(element);
    return { word, link, button, count: () => pressed };
  }

  /** A finger's event of `type` on `target`: whether it was left to the browser. */
  function finger(target: HTMLElement, type: string, pointerType = 'touch'): boolean {
    const event = new Event(type, { bubbles: true, cancelable: true });
    for (const [name, value] of Object.entries({
      pointerType,
      isPrimary: true,
      clientX: 0,
      clientY: 0,
    })) {
      Object.defineProperty(event, name, { value });
    }
    return target.dispatchEvent(event);
  }

  afterEach(() => {
    vi.useRealTimers();
  });

  it('on the comment answers it, and keeps the click that ends it from what was pressed', () => {
    vi.useFakeTimers();
    const { word, count } = comment();
    finger(word, 'pointerdown');
    vi.advanceTimersByTime(LONG_PRESS_MS);
    expect(count()).toBe(1);
    expect(finger(word, 'contextmenu')).toBe(false);
    finger(word, 'pointerup');
    expect(finger(word, 'click')).toBe(false);
  });

  it('that ended in no click keeps none from the next press, on a link or with a mouse', () => {
    vi.useFakeTimers();
    const { word, link, count } = comment();
    for (const pointer of ['touch', 'mouse']) {
      finger(word, 'pointerdown');
      vi.advanceTimersByTime(LONG_PRESS_MS);
      // The browser takes the finger over: no click ends the press.
      finger(word, 'pointercancel');
      finger(link, 'pointerdown', pointer);
      finger(link, 'pointerup', pointer);
      expect(finger(link, 'click', pointer)).toBe(true);
    }
    expect(count()).toBe(2);
  });

  it('taken over by the browser keeps its menu, and no click of a key that follows', () => {
    vi.useFakeTimers();
    const { word, button, count } = comment();
    finger(word, 'pointerdown');
    vi.advanceTimersByTime(LONG_PRESS_MS);
    finger(word, 'pointercancel');
    expect(finger(word, 'contextmenu')).toBe(false);
    // Enter on the button: a click with no press before it.
    expect(button.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))).toBe(
      true,
    );
    expect(count()).toBe(1);
  });

  it('on a link or a button inside leaves them as they are', () => {
    vi.useFakeTimers();
    const { link, button, count } = comment();
    for (const target of [link, button]) {
      finger(target, 'pointerdown');
      vi.advanceTimersByTime(LONG_PRESS_MS);
      expect(finger(target, 'contextmenu')).toBe(true);
      finger(target, 'pointerup');
      expect(finger(target, 'click')).toBe(true);
    }
    expect(count()).toBe(0);
  });
});
