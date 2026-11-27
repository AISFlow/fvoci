/** The value of a person property: one person, picked from the people the caller passes. */
import { describe, expect, it } from 'vitest';

import { emptyName } from '$lib/logic/person-names.ts';

import {
  closePicker,
  fireEvent,
  mount,
  press,
  screen,
  type,
  USERS,
  valueButton,
  within,
} from './support.ts';

const button = (): HTMLElement => valueButton('My person');
const search = (): HTMLInputElement => screen.getByRole('combobox', { name: 'My person' });
const list = (): HTMLElement => screen.getByRole('listbox', { name: 'My person' });
const row = (name: string | RegExp): HTMLElement => within(list()).getByRole('option', { name });
const names = (): (string | null)[] =>
  within(list())
    .queryAllByRole('option')
    .map((option) => option.textContent.trim());
const isOpen = (): boolean => button().getAttribute('aria-expanded') === 'true';

async function open(): Promise<void> {
  await fireEvent.click(button());
}

describe('a person property, closed', () => {
  it('shows the person by their display name and login name', () => {
    mount('person', 'user-2');
    expect(button()).toHaveAccessibleName('My person Mino (@minho)');
  });

  it('shows the login name alone when there is no display name, and follows a new one', async () => {
    const mounted = mount('person', 'user-1');
    expect(button()).toHaveAccessibleName('My person jisoo');
    await mounted.setProps({ users: [{ id: 'user-1', username: 'jisoo', displayName: '김지수' }] });
    expect(button()).toHaveAccessibleName('My person 김지수 (@jisoo)');
  });

  it('shows a placeholder without a value', () => {
    mount('person', '');
    expect(within(button()).getByText('Empty')).toBeInTheDocument();
  });

  it('shows someone who is not among the people as an unknown user', () => {
    mount('person', 'user-gone');
    expect(button()).toHaveAccessibleName('My person Unknown user');
  });

  it('names someone known though not offered, and does not offer them', async () => {
    mount('person', 'user-gone', {
      named: [{ id: 'user-gone', username: 'leaver', displayName: '' }],
    });
    expect(button()).toHaveAccessibleName('My person leaver');
    await open();
    expect(names()).not.toContain('leaver');
    expect(names()).toHaveLength(USERS.length);
  });
});

describe('a person property, opened', () => {
  it('lists the people and marks the one in the value', async () => {
    mount('person', 'user-2');
    await open();
    expect(names()).toEqual(['jisoo', 'Mino (@minho)', 'ada']);
    expect(row('Mino (@minho)')).toHaveAttribute('aria-selected', 'true');
  });

  it('has no entry for the reader themselves: the list is the people passed in', async () => {
    mount('person', '');
    await open();
    expect(names()).toEqual(['jisoo', 'Mino (@minho)', 'ada']);
    expect(USERS).toHaveLength(3);
    expect(within(list()).queryByText('Me')).toBeNull();
  });

  it('stores the person that is pressed, once, and closes', async () => {
    const { onChange } = mount('person', '');
    await open();
    await fireEvent.click(row('ada'));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('user-3');
    expect(isOpen()).toBe(false);
  });

  it('closes without storing when the same person is pressed', async () => {
    const { onChange } = mount('person', 'user-3');
    await open();
    await fireEvent.click(row('ada'));
    expect(onChange).not.toHaveBeenCalled();
    expect(isOpen()).toBe(false);
  });

  it('narrows the list by login name and display name', async () => {
    mount('person', '');
    await open();
    await type(search(), 'MINO');
    expect(names()).toEqual(['Mino (@minho)']);
    await type(search(), 'minh');
    expect(names()).toEqual(['Mino (@minho)']);
    await type(search(), 'JIS');
    expect(names()).toEqual(['jisoo']);
  });

  it('finds a person by any part of their name, though shown by another', async () => {
    mount('person', '', {
      users: [
        {
          id: 'user-4',
          username: 'ykim',
          displayName: '김영환',
          name: {
            ...emptyName(),
            lastName: '김',
            firstName: '영환',
            latinFirstName: 'Younghwan',
            latinLastName: 'Kim',
          },
        },
        ...USERS,
      ],
    });
    await open();
    for (const wanted of ['younghwan', 'Kim Young', '영환']) {
      await type(search(), wanted);
      expect(names(), wanted).toEqual(['김영환 (@ykim)']);
    }
  });

  it('says so when nobody matches, and offers nothing to create', async () => {
    mount('person', '');
    await open();
    await type(search(), 'nobody');
    expect(names()).toEqual([]);
    expect(screen.getByRole('status')).toHaveTextContent('No matches');
  });

  it('picks with the keyboard', async () => {
    const { onChange } = mount('person', '');
    await open();
    await press(search(), 'ArrowDown');
    await press(search(), 'Enter');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('user-2');
  });

  it('clears the value', async () => {
    const { onChange } = mount('person', 'user-2');
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
    expect(isOpen()).toBe(false);
  });

  it('can clear an unknown user', async () => {
    const { onChange } = mount('person', 'user-gone');
    await open();
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith('');
  });

  it('closes on Escape and stores nothing', async () => {
    const { onChange } = mount('person', 'user-2');
    await open();
    await closePicker();
    expect(isOpen()).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('a person property that may not be edited', () => {
  it('shows the name and cannot be opened', () => {
    const { container, onChange } = mount('person', 'user-2', { readOnly: true });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('minho');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is the same in display mode', () => {
    const { container } = mount('person', 'user-2', { mode: 'display' });
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('minho');
  });

  it('shows the placeholder, or nothing, without a value', async () => {
    const mounted = mount('person', '', { readOnly: true });
    expect(mounted.container).toHaveTextContent('Empty');
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(mounted.container).toHaveTextContent('');
  });
});

describe('a person property that starts in the editing state', () => {
  it('has its list open at once', () => {
    mount('person', '', { startOpen: true });
    expect(names()).toEqual(['jisoo', 'Mino (@minho)', 'ada']);
  });
});
