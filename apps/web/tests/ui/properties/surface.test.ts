/**
 * How pickers open: one argument, `presentation`, decides between a popover
 * and a sheet for every kind that has a picker, and what a person can do in
 * the picker is the same on both.
 */
import { describe, expect, it } from 'vitest';

import type { PropertyValue } from '$lib/logic/wire.ts';

import { closePicker, fireEvent, mount, screen, tick, valueButton, within } from './support.ts';

const PICKERS: ['select' | 'multiSelect' | 'person' | 'multiPerson' | 'date', PropertyValue][] = [
  ['select', 'option-1'],
  ['multiSelect', ['option-1']],
  ['person', 'user-1'],
  ['multiPerson', ['user-1']],
  ['date', { from: '2026-03-05' }],
];

describe.each(PICKERS)('the picker of a %s property', (type, value) => {
  const name = `My ${type}`;

  it.each(['popover', 'sheet'] as const)(
    'keeps the look of the value button whatever the %s adds to it',
    (presentation) => {
      // What opens the surface is given attributes by it; the button's own must win.
      mount(type, value, { presentation });
      expect(valueButton(name).getAttribute('data-slot')).toMatch(/^property-/);
      expect(valueButton(name)).toHaveClass('flex', 'w-full', 'text-left');
      expect(valueButton(name)).toHaveAttribute('aria-haspopup', 'dialog');
    },
  );

  it('opens as a popover next to the value on a wide screen', async () => {
    mount(type, value, { presentation: 'popover' });
    await fireEvent.click(valueButton(name));
    const surface = document.querySelector('[data-presentation]');
    expect(surface).toHaveAttribute('data-presentation', 'popover');
    expect(surface?.closest('[data-slot="popover-content"]')).toHaveAttribute('aria-label', name);
    expect(document.querySelector('[data-slot="sheet-content"]')).toBeNull();
  });

  it('opens as a sheet from the bottom, headed by the name of the property, on a narrow one', async () => {
    mount(type, value, { presentation: 'sheet' });
    await fireEvent.click(valueButton(name));
    const sheet = screen.getByRole('dialog', { name });
    expect(sheet).toHaveAttribute('data-slot', 'sheet-content');
    expect(sheet).toHaveAttribute('data-side', 'bottom');
    expect(within(sheet).getByRole('heading', { name })).toBeInTheDocument();
    expect(sheet.querySelector('[data-presentation]')).toHaveAttribute(
      'data-presentation',
      'sheet',
    );
    expect(document.querySelector('[data-slot="popover-content"]')).toBeNull();
  });

  it('keeps the sheet clear of each edge of the screen by the inset of that edge', async () => {
    // The left and the right inset differ on some devices (a camera on one
    // side, a strip the system keeps on the other): one value for both would
    // be wrong for one of them.
    mount(type, value, { presentation: 'sheet' });
    await fireEvent.click(valueButton(name));
    const sheet = screen.getByRole('dialog', { name });
    expect(sheet).toHaveClass('pl-(--safe-left)', 'pr-(--safe-right)', 'pb-(--safe-bottom)');
  });

  it('closes the sheet with its Done button and with Escape', async () => {
    mount(type, value, { presentation: 'sheet' });
    await fireEvent.click(valueButton(name));
    await fireEvent.click(screen.getByRole('button', { name: 'Done' }));
    await tick();
    expect(valueButton(name)).toHaveAttribute('aria-expanded', 'false');
    await fireEvent.click(valueButton(name));
    expect(valueButton(name)).toHaveAttribute('aria-expanded', 'true');
    await closePicker();
    expect(valueButton(name)).toHaveAttribute('aria-expanded', 'false');
  });

  it('defaults to the popover', async () => {
    mount(type, value);
    await fireEvent.click(valueButton(name));
    expect(document.querySelector('[data-presentation]')).toHaveAttribute(
      'data-presentation',
      'popover',
    );
  });
});

describe('picking on a sheet', () => {
  it('stores a picked option like the popover does', async () => {
    const { onChange } = mount('select', '', { presentation: 'sheet' });
    await fireEvent.click(valueButton('My select'));
    const sheet = screen.getByRole('dialog', { name: 'My select' });
    await fireEvent.click(within(sheet).getByRole('option', { name: 'Done' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('option-3');
    expect(valueButton('My select')).toHaveAttribute('aria-expanded', 'false');
  });

  it('stores picked people like the popover does', async () => {
    const { onChange } = mount('multiPerson', ['user-1'], { presentation: 'sheet' });
    await fireEvent.click(valueButton('My multiPerson'));
    const sheet = screen.getByRole('dialog', { name: 'My multiPerson' });
    await fireEvent.click(within(sheet).getByRole('option', { name: 'ada' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-1', 'user-3']);
  });
});
