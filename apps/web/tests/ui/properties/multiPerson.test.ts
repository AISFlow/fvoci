/** The value of a multi-person property: several people; the list stays open while it changes. */
import { describe, expect, it } from 'vitest';

import {
  closePicker,
  fireEvent,
  mount,
  press,
  screen,
  type,
  valueButton,
  within,
} from './support.ts';

const button = (): HTMLElement => valueButton('My multiPerson');
const search = (): HTMLInputElement => screen.getByRole('combobox', { name: 'My multiPerson' });
const list = (): HTMLElement => screen.getByRole('listbox', { name: 'My multiPerson' });
const row = (name: string | RegExp): HTMLElement => within(list()).getByRole('option', { name });
const isOpen = (): boolean => button().getAttribute('aria-expanded') === 'true';
const chips = (): (string | null)[] =>
  [...button().querySelectorAll('[data-slot="person-chip"]')].map((chip) =>
    chip.textContent.trim(),
  );

async function open(): Promise<void> {
  await fireEvent.click(button());
}

describe('a multi-person property, closed', () => {
  it('shows the people in the order of the value', () => {
    mount('multiPerson', ['user-3', 'user-1']);
    expect(chips()).toEqual(['ada', 'jisoo']);
  });

  it('shows a placeholder without a value', () => {
    mount('multiPerson', []);
    expect(within(button()).getByText('Empty')).toBeInTheDocument();
  });

  it('shows someone who is not among the people as an unknown user, and each person once', () => {
    mount('multiPerson', ['user-1', 'user-gone', 'user-1']);
    expect(chips()).toEqual(['jisoo', 'Unknown user']);
  });

  it('is empty for a value that is not a list', () => {
    mount('multiPerson', 'user-1');
    expect(chips()).toEqual([]);
  });
});

describe('a multi-person property, opened', () => {
  it('lists the people and marks those in the value', async () => {
    mount('multiPerson', ['user-1']);
    await open();
    expect(list()).toHaveAttribute('aria-multiselectable', 'true');
    expect(row('jisoo')).toHaveAttribute('aria-selected', 'true');
    expect(row('Mino (@minho)')).toHaveAttribute('aria-selected', 'false');
  });

  it('adds the person that is pressed and stays open', async () => {
    const { onChange } = mount('multiPerson', ['user-1']);
    await open();
    await fireEvent.click(row('ada'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-1', 'user-3']);
    expect(isOpen()).toBe(true);
  });

  it('takes out a person in the value that is pressed in the list', async () => {
    const { onChange } = mount('multiPerson', ['user-1', 'user-3']);
    await open();
    await fireEvent.click(row('jisoo'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-3']);
  });

  it('takes out a person with the button next to their name', async () => {
    const { onChange } = mount('multiPerson', ['user-1', 'user-3']);
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove ada' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-1']);
    expect(isOpen()).toBe(true);
  });

  it('takes out an unknown user the same way, keeping everyone else', async () => {
    const { onChange } = mount('multiPerson', ['user-1', 'user-gone']);
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove Unknown user' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-1']);
  });

  it('keeps an unknown user in the value while others are added', async () => {
    const { onChange } = mount('multiPerson', ['user-gone']);
    await open();
    await fireEvent.click(row('ada'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-gone', 'user-3']);
  });

  it('takes out the last person on Backspace in the empty field', async () => {
    const { onChange } = mount('multiPerson', ['user-1', 'user-3']);
    await open();
    await press(search(), 'Backspace');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-1']);
  });

  it('narrows the list by typing and picks with Enter', async () => {
    const { onChange } = mount('multiPerson', ['user-1']);
    await open();
    await type(search(), 'mino');
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith(['user-1', 'user-2']);
    expect(search()).toHaveValue('');
  });

  it('clears the value and stays open', async () => {
    const { onChange } = mount('multiPerson', ['user-1', 'user-3']);
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith([]);
    expect(isOpen()).toBe(true);
  });

  it('closes on Escape and stores nothing more', async () => {
    const { onChange } = mount('multiPerson', ['user-1']);
    await open();
    await closePicker();
    expect(isOpen()).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('a multi-person property that may not be edited', () => {
  it('shows the names and cannot be opened', () => {
    const { container, onChange } = mount('multiPerson', ['user-1', 'user-2'], { readOnly: true });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('jisoo');
    expect(container).toHaveTextContent('minho');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is the same in display mode', () => {
    const { container } = mount('multiPerson', ['user-1'], { mode: 'display' });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('jisoo');
  });
});

describe('a multi-person property that starts in the editing state', () => {
  it('has its list open at once', () => {
    mount('multiPerson', [], { startOpen: true });
    expect(within(list()).getAllByRole('option')).toHaveLength(3);
  });
});
