/**
 * A finger's press that opens something from a body (touch-press.ts): kept
 * from the browser and the editor while the body is not being written, left
 * as it is while it is, and for a mouse.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { keepTouchFromText } from '$lib/editor/touch-press.ts';

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
