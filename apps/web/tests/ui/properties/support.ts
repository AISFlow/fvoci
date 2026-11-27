/**
 * What the tests of the property components share: a card, people, readers
 * in different time zones, and the few gestures a test makes (typing into a
 * field, opening a picker, pressing a key).
 *
 * The tests run in a simulated page without layout. Moments are built with
 * `at(zone, ...)` or `Date.UTC`, never from the time zone of the machine:
 * the suite runs under three of them.
 */
import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, vi, type Mock } from 'vitest';

import PropertyValue from '$lib/components/properties/PropertyValue.svelte';
import type { PropertyUser, PropertyValueProps } from '$lib/components/properties/types.ts';
import type { Regional } from '$lib/logic/regional.ts';
import {
  createCard,
  type Card,
  type PropertyOption,
  type PropertyTemplate,
  type PropertyType,
  type PropertyValue as StoredValue,
} from '$lib/logic/wire.ts';

import { at } from '../../support/zones.ts';

// The project's setup file does not clear the page between tests; every test
// file of this folder imports this module, so it is done here.
afterEach(() => {
  cleanup();
});

/** A reader in Seoul who writes hours to twelve and starts the week on Sunday. */
export const SEOUL: Regional = { timeZone: 'Asia/Seoul', weekStart: 7, hourCycle: 'h12' };
/** A reader whose account is set to New York. */
export const NEW_YORK: Regional = { timeZone: 'America/New_York', weekStart: 7, hourCycle: 'h12' };
/** A reader in Berlin: hours to 23, weeks from Monday. */
export const BERLIN: Regional = { timeZone: 'Europe/Berlin', weekStart: 1, hourCycle: 'h23' };

/** "Now" in every test: noon on 10 March 2026 in Seoul. */
export const NOW = at('Asia/Seoul', 2026, 3, 10, 12, 0);

export const USERS: PropertyUser[] = [
  { id: 'user-1', username: 'jisoo', displayName: '' },
  { id: 'user-2', username: 'minho', displayName: 'Mino' },
  { id: 'user-3', username: 'ada', displayName: '' },
];

export const OPTIONS: PropertyOption[] = [
  { id: 'option-1', label: 'To do', color: 'default' },
  { id: 'option-2', label: 'In progress', color: 'blue' },
  { id: 'option-3', label: 'Done', color: 'green' },
];

export function templateOf(
  type: PropertyType,
  overrides: Partial<PropertyTemplate> = {},
): PropertyTemplate {
  return {
    id: `property-${type}`,
    name: `My ${type}`,
    type,
    options: type === 'select' || type === 'multiSelect' ? OPTIONS : [],
    ...overrides,
  };
}

/** A card made by the first user at 09:00 on 5 March 2026 in Seoul and changed by the second a day later. */
export function testCard(): Card {
  const card = createCard();
  card.id = 'card-1';
  card.title = 'A card';
  card.createdBy = 'user-1';
  card.modifiedBy = 'user-2';
  card.createdAt = at('Asia/Seoul', 2026, 3, 5, 9, 0);
  card.updatedAt = at('Asia/Seoul', 2026, 3, 6, 18, 30);
  return card;
}

type Overrides = Partial<Omit<PropertyValueProps, 'template' | 'value'>>;

export interface Mounted {
  onChange: Mock<(value: StoredValue | undefined) => void>;
  container: HTMLElement;
  /** Gives the component another stored value, as the caller would after saving. */
  setValue: (value: StoredValue | undefined) => Promise<void>;
  setProps: (props: Partial<PropertyValueProps>) => Promise<void>;
  unmount: () => void;
}

/** Draws the value of a property of the given kind, for a reader in Seoul unless said otherwise. */
export function mount(
  type: PropertyType | PropertyTemplate,
  value: StoredValue | undefined,
  overrides: Overrides = {},
): Mounted {
  const onChange = vi.fn<(value: StoredValue | undefined) => void>();
  const props: PropertyValueProps = {
    template: typeof type === 'string' ? templateOf(type) : type,
    card: testCard(),
    value,
    users: USERS,
    locale: 'en-US',
    regional: SEOUL,
    now: NOW,
    showEmptyPlaceholder: true,
    onChange,
    ...overrides,
  };
  const { container, rerender, unmount } = render(PropertyValue, { props });
  return {
    onChange,
    container,
    setValue: (next) => rerender({ value: next }),
    setProps: (next) => rerender(next),
    unmount,
  };
}

/** Replaces the text of a field, the way typing over it does. */
export async function type(field: HTMLElement, text: string): Promise<void> {
  field.focus();
  await fireEvent.input(field, { target: { value: text } });
}

export async function press(
  target: Element,
  key: string,
  init: KeyboardEventInit = {},
): Promise<void> {
  await fireEvent.keyDown(target, { key, ...init });
}

export async function leave(field: HTMLElement): Promise<void> {
  field.blur();
  await tick();
}

/** The button that shows the value of a property and opens its picker. */
export function valueButton(name: string | RegExp): HTMLElement {
  return screen.getByRole('button', {
    name: typeof name === 'string' ? new RegExp(`^${escape(name)}`) : name,
  });
}

const escape = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Closes an open picker the way pressing Escape does. */
export async function closePicker(): Promise<void> {
  await fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
  await tick();
}

/** The cell of a day in the open calendar. */
export function dayCell(date: string): HTMLElement {
  const cell = document.querySelector<HTMLElement>(`[data-bits-day][data-value="${date}"]`);
  if (!cell) throw new Error(`the calendar does not show ${date}`);
  return cell;
}

export { at, fireEvent, screen, tick, within };
