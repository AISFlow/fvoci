/**
 * The value of a date property: a day, a moment, or a range, and the wire
 * value each edit produces.
 *
 * A day is the same for every reader. A moment is entered and shown on the
 * clock of the reader's account time zone, which the tests pass in; the
 * suite itself runs with the machine in three other time zones, so a result
 * that followed the machine instead would fail in at least one of them.
 */
import { describe, expect, it } from 'vitest';

import type { PropertyValueProps } from '$lib/components/properties/types.ts';
import type { Regional } from '$lib/logic/regional.ts';

import {
  at,
  BERLIN,
  closePicker,
  dayCell,
  fireEvent,
  mount,
  NEW_YORK,
  press,
  screen,
  SEOUL,
  tick,
  type,
  valueButton,
  within,
} from './support.ts';

const button = (): HTMLElement => valueButton('My date');
const isOpen = (): boolean => button().getAttribute('aria-expanded') === 'true';
const shownText = (): string => button().textContent.replace('My date', '').trim();
const toggle = (name: string): HTMLElement => screen.getByRole('switch', { name });
const startField = (): HTMLInputElement =>
  screen.getByRole('textbox', { name: /^(My date|Start date)$/ });
const endField = (): HTMLInputElement => screen.getByRole('textbox', { name: 'End date' });
const times = (name: string): HTMLElement => screen.getByRole('group', { name });
const hour = (group = 'Time'): HTMLSelectElement =>
  within(times(group)).getByRole('combobox', { name: 'Hour' });
const minute = (group = 'Time'): HTMLSelectElement =>
  within(times(group)).getByRole('combobox', { name: 'Minute' });
const period = (group = 'Time'): HTMLSelectElement =>
  within(times(group)).getByRole('combobox', { name: 'AM or PM' });

async function open(): Promise<void> {
  await fireEvent.click(button());
}

async function pick(date: string): Promise<void> {
  await fireEvent.click(dayCell(date));
}

async function choose(select: HTMLSelectElement, value: string | number): Promise<void> {
  await fireEvent.change(select, { target: { value: String(value) } });
  await tick();
}

/** Opens the picker of a value, for a reader, and gives back what was mounted. */
async function edit(
  value: PropertyValueProps['value'],
  regional: Regional = SEOUL,
  overrides: Partial<PropertyValueProps> = {},
): Promise<ReturnType<typeof mount>> {
  const mounted = mount('date', value, { regional, ...overrides });
  await open();
  return mounted;
}

describe('a date property, closed', () => {
  it('shows a placeholder without a value, when asked to', async () => {
    const mounted = mount('date', undefined);
    expect(shownText()).toBe('Empty');
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(shownText()).toBe('');
  });

  it('shows a day of this year without the year', () => {
    mount('date', { from: '2026-03-05' });
    expect(shownText()).toBe('March 05');
  });

  it('shows the year of a day in another year', () => {
    mount('date', { from: '2021-06-15' });
    expect(shownText()).toBe('June 15, 2021');
  });

  it('shows a day in the language of the reader', () => {
    mount('date', { from: '2026-06-15' }, { locale: 'es' });
    expect(shownText()).toBe('15 de junio');
  });

  it('shows a range from its first to its last day', () => {
    mount('date', { from: '2026-03-05', to: '2026-03-09' });
    expect(shownText()).toBe('March 05 → March 09');
  });

  it('shows a moment on the clock of the account, twelve hours or twenty-four', async () => {
    const moment = at('Asia/Seoul', 2026, 3, 5, 15, 20);
    const mounted = mount('date', { from: moment });
    expect(shownText()).toBe('March 05 at 3:20 PM');
    await mounted.setProps({ regional: { ...SEOUL, hourCycle: 'h23' } });
    expect(shownText()).toBe('March 05 at 15:20');
  });

  it('shows the same moment as another day and time to a reader whose account is elsewhere', () => {
    mount('date', { from: at('Asia/Seoul', 2026, 3, 5, 9, 0) }, { regional: NEW_YORK });
    // The machine's time zone plays no part: this is 19:00 the evening before in New York.
    expect(shownText()).toBe('March 04 at 7:00 PM');
  });

  it('shows a day without a time as the same day to every reader', () => {
    for (const regional of [SEOUL, NEW_YORK, BERLIN]) {
      const { unmount } = mount('date', { from: '2026-03-05' }, { regional });
      expect(shownText()).toBe('March 05');
      unmount();
    }
  });

  it('shows the value it is given next', async () => {
    const mounted = mount('date', undefined);
    await mounted.setValue({ from: '2026-06-15' });
    expect(shownText()).toBe('June 15');
  });

  it('treats what is not a date value as no value', () => {
    mount('date', 'next week');
    expect(shownText()).toBe('Empty');
  });
});

describe('picking a day', () => {
  it('stores the day that is pressed when the picker closes, once', async () => {
    const { onChange } = await edit(undefined);
    await pick('2026-03-15');
    expect(onChange).not.toHaveBeenCalled();
    expect(shownText()).toBe('March 15');
    await closePicker();
    expect(isOpen()).toBe(false);
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-15' });
  });

  it('stores nothing when nothing was changed', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('stores nothing when the same day is pressed again', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await pick('2026-03-05');
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('replaces a day by the next one pressed', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await pick('2026-03-15');
    await pick('2026-03-20');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-20' });
  });

  it('opens on the month of the value, and on this month without one', async () => {
    const first = await edit({ from: '2021-06-15' });
    expect(dayCell('2021-06-15')).toHaveAttribute('data-selected');
    first.unmount();
    await edit(undefined);
    expect(dayCell('2026-03-10')).toBeInTheDocument();
  });

  it('stores the same day whatever the reader’s time zone', async () => {
    for (const regional of [SEOUL, NEW_YORK, BERLIN]) {
      const mounted = await edit({ from: '2026-03-05' }, regional);
      await pick('2026-03-20');
      await closePicker();
      expect(mounted.onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-20' });
      mounted.unmount();
    }
  });

  it('keeps what was picked when the component is removed with the picker open', async () => {
    const mounted = await edit(undefined);
    await pick('2026-03-15');
    mounted.unmount();
    expect(mounted.onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-15' });
  });
});

describe('today', () => {
  it('is the day the current moment falls on in the account’s time zone', async () => {
    // Noon on the 10th in Seoul is still the evening of the 9th in New York.
    const seoul = await edit(undefined, SEOUL);
    expect(dayCell('2026-03-10')).toHaveAttribute('data-reader-today');
    expect(dayCell('2026-03-09')).not.toHaveAttribute('data-reader-today');
    seoul.unmount();
    await edit(undefined, NEW_YORK);
    expect(dayCell('2026-03-09')).toHaveAttribute('data-reader-today');
    expect(dayCell('2026-03-10')).not.toHaveAttribute('data-reader-today');
  });

  it('is stored by the Today button', async () => {
    const { onChange } = await edit(undefined, SEOUL);
    await fireEvent.click(screen.getByRole('button', { name: 'Today' }));
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-10' });
  });

  it('is another day for a reader whose account is in another time zone', async () => {
    const { onChange } = await edit(undefined, NEW_YORK);
    await fireEvent.click(screen.getByRole('button', { name: 'Today' }));
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-09' });
  });

  it('brings the calendar back to this month', async () => {
    await edit({ from: '2021-06-15' });
    await fireEvent.click(screen.getByRole('button', { name: 'Today' }));
    expect(dayCell('2026-03-10')).toHaveAttribute('data-selected');
  });
});

describe('the first day of the week', () => {
  const firstHeader = (): string | null => document.querySelector('th')?.textContent.trim() ?? null;

  it('is the account’s: Sunday for one reader, Monday for another', async () => {
    const sunday = await edit(undefined, SEOUL);
    expect(firstHeader()).toBe('Su');
    sunday.unmount();
    const monday = await edit(undefined, BERLIN);
    expect(firstHeader()).toBe('Mo');
    monday.unmount();
    await edit(undefined, { ...SEOUL, weekStart: 6 });
    expect(firstHeader()).toBe('Sa');
  });
});

describe('a range', () => {
  it('is off for a single day and on for a range', async () => {
    const single = await edit({ from: '2026-03-05' });
    expect(toggle('End date')).not.toBeChecked();
    expect(screen.queryByRole('textbox', { name: 'End date' })).toBeNull();
    single.unmount();
    await edit({ from: '2026-03-05', to: '2026-03-09' });
    expect(toggle('End date')).toBeChecked();
    expect(endField()).toHaveValue('03/09/2026');
  });

  it('is made by picking a day, turning the end date on and picking the end', async () => {
    const { onChange } = await edit(undefined);
    await pick('2026-03-15');
    await fireEvent.click(toggle('End date'));
    await pick('2026-03-20');
    expect(shownText()).toBe('March 15 → March 20');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-15', to: '2026-03-20' });
  });

  it('runs forwards when the second day pressed is before the first', async () => {
    const { onChange } = await edit({ from: '2026-03-15' });
    await fireEvent.click(toggle('End date'));
    await pick('2026-03-11');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-11', to: '2026-03-15' });
  });

  it('is the one day when the end date is turned on and no end is picked', async () => {
    const { onChange } = await edit({ from: '2026-03-15' });
    await fireEvent.click(toggle('End date'));
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('starts over with the next day pressed once it has both ends', async () => {
    const { onChange } = await edit({ from: '2026-03-05', to: '2026-03-09' });
    await pick('2026-03-16');
    expect(shownText()).toBe('March 16');
    await pick('2026-03-18');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-16', to: '2026-03-18' });
  });

  it('loses its end when the end date is turned off', async () => {
    const { onChange } = await edit({ from: '2026-03-05', to: '2026-03-09' });
    await fireEvent.click(toggle('End date'));
    expect(shownText()).toBe('March 05');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-05' });
  });

  it('is completed by the Today button when only its start is picked', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await fireEvent.click(toggle('End date'));
    await fireEvent.click(screen.getByRole('button', { name: 'Today' }));
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-05', to: '2026-03-10' });
  });
});

describe('clearing', () => {
  it('stores an empty value', async () => {
    const { onChange } = await edit({ from: '2026-06-15' });
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(shownText()).toBe('Empty');
    expect(isOpen()).toBe(true);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith(undefined);
  });

  it('clears a range with times, and turns both off', async () => {
    const { onChange } = await edit({
      from: at('Asia/Seoul', 2026, 3, 5, 9, 0),
      to: at('Asia/Seoul', 2026, 3, 9, 18, 0),
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(toggle('End date')).not.toBeChecked();
    expect(toggle('Include time')).not.toBeChecked();
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith(undefined);
  });

  it('has nothing to clear without a value', async () => {
    const { onChange } = await edit(undefined);
    expect(screen.getByRole('button', { name: 'Clear' })).toBeDisabled();
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('typing a date', () => {
  const range = { from: '2021-06-15', to: '2021-06-20' };

  it('shows the dates the way the language writes them', async () => {
    await edit(range);
    expect(startField()).toHaveValue('06/15/2021');
    expect(endField()).toHaveValue('06/20/2021');
  });

  it('sets both ends of a range on Enter', async () => {
    const { onChange } = await edit(range);
    await type(startField(), '07/15/2021');
    await press(startField(), 'Enter');
    await type(endField(), '07/20/2021');
    await press(endField(), 'Enter');
    expect(shownText()).toBe('July 15, 2021 → July 20, 2021');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2021-07-15', to: '2021-07-20' });
  });

  it('reads the numbers in the order of the reader’s language', async () => {
    const { onChange } = await edit(range, SEOUL, { locale: 'es' });
    expect(startField()).toHaveValue('15/06/2021');
    await type(startField(), '15/07/2021');
    await press(startField(), 'Enter');
    await type(endField(), '20/07/2021');
    await press(endField(), 'Enter');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2021-07-15', to: '2021-07-20' });
  });

  it('puts the date back on Escape, keeps the picker open, and stores nothing', async () => {
    const { onChange } = await edit(range);
    await type(startField(), '07/15/2021');
    await press(startField(), 'Escape');
    await type(endField(), '07/20/2021');
    await press(endField(), 'Escape');
    expect(startField()).toHaveValue('06/15/2021');
    expect(endField()).toHaveValue('06/20/2021');
    expect(isOpen()).toBe(true);
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('settles a typed date on leaving the field', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await type(startField(), '3/20/2026');
    startField().blur();
    await tick();
    expect(startField()).toHaveValue('03/20/2026');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-20' });
  });

  it('counts a date still being typed when the picker closes', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await fireEvent.input(startField(), { target: { value: '2026-04-01' } });
    await fireEvent.keyDown(document.body, { key: 'Escape' });
    await tick();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-04-01' });
  });

  it('puts back the date when what was typed is not one', async () => {
    const { onChange } = await edit({ from: '2026-03-05' });
    await type(startField(), '02/30/2026');
    await press(startField(), 'Enter');
    expect(startField()).toHaveValue('03/05/2026');
    await type(startField(), 'soon');
    await press(startField(), 'Enter');
    expect(startField()).toHaveValue('03/05/2026');
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('gives an empty value its first day', async () => {
    const { onChange } = await edit(undefined);
    await type(startField(), '12/25/2026');
    await press(startField(), 'Enter');
    expect(dayCell('2026-12-25')).toHaveAttribute('data-selected');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-12-25' });
  });

  it('moves the start along when the end is typed before it', async () => {
    const { onChange } = await edit({ from: '2026-03-05', to: '2026-03-09' });
    await type(endField(), '03/01/2026');
    await press(endField(), 'Enter');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-01' });
  });

  it('moves the end along when the start is typed after it', async () => {
    const { onChange } = await edit({ from: '2026-03-05', to: '2026-03-09' });
    await type(startField(), '03/12/2026');
    await press(startField(), 'Enter');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-12' });
  });
});

describe('including a time', () => {
  it('is off for a day and on for a moment', async () => {
    const day = await edit({ from: '2026-03-05' });
    expect(toggle('Include time')).not.toBeChecked();
    expect(screen.queryByRole('group', { name: 'Time' })).toBeNull();
    day.unmount();
    await edit({ from: at('Asia/Seoul', 2026, 3, 5, 15, 20) });
    expect(toggle('Include time')).toBeChecked();
    expect(hour()).toHaveValue('3');
    expect(minute()).toHaveValue('20');
    expect(period()).toHaveValue('pm');
  });

  it('turns a day into nine in the morning of that day on the account’s clock', async () => {
    const { onChange } = await edit({ from: '2026-03-05' }, SEOUL);
    await fireEvent.click(toggle('Include time'));
    expect(shownText()).toBe('March 05 at 9:00 AM');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: at('Asia/Seoul', 2026, 3, 5, 9, 0) });
  });

  it('means nine in New York for a reader whose account is set to New York', async () => {
    const { onChange } = await edit({ from: '2026-03-05' }, NEW_YORK);
    await fireEvent.click(toggle('Include time'));
    await closePicker();
    const stored = at('America/New_York', 2026, 3, 5, 9, 0);
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: stored });
    // Fourteen hours after nine in Seoul: not the same moment.
    expect(stored - at('Asia/Seoul', 2026, 3, 5, 9, 0)).toBe(14 * 3_600_000);
  });

  it('says which time zone the times are in', async () => {
    await edit({ from: at('Asia/Seoul', 2026, 3, 5, 9, 0) }, NEW_YORK);
    expect(screen.getByText('Times are in America/New_York')).toBeInTheDocument();
  });

  it('stores the hour, minute and half of the day that are chosen', async () => {
    const { onChange } = await edit({ from: at('Asia/Seoul', 2026, 3, 5, 9, 0) });
    await choose(hour(), 3);
    await choose(minute(), 45);
    await choose(period(), 'pm');
    expect(shownText()).toBe('March 05 at 3:45 PM');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('Asia/Seoul', 2026, 3, 5, 15, 45),
    });
  });

  it('reads twelve at night and twelve at noon the way a twelve-hour clock does', async () => {
    const { onChange } = await edit({ from: at('Asia/Seoul', 2026, 3, 5, 0, 5) });
    expect(hour()).toHaveValue('12');
    expect(period()).toHaveValue('am');
    await choose(period(), 'pm');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: at('Asia/Seoul', 2026, 3, 5, 12, 5) });
  });

  it('offers the hours 0 to 23, and no halves of the day, to a reader who counts to 23', async () => {
    const { onChange } = await edit({ from: at('Europe/Berlin', 2026, 3, 5, 15, 20) }, BERLIN);
    expect(hour().options).toHaveLength(24);
    expect(hour()).toHaveValue('15');
    expect(within(times('Time')).queryByRole('combobox', { name: 'AM or PM' })).toBeNull();
    await choose(hour(), 23);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('Europe/Berlin', 2026, 3, 5, 23, 20),
    });
  });

  it('names the halves of the day in the reader’s language, before the hour where that is the order', async () => {
    await edit({ from: at('Asia/Seoul', 2026, 3, 5, 15, 20) }, SEOUL, { locale: 'ko' });
    const group = document.querySelector('[data-slot="time-fields"]');
    const selects = [...(group?.querySelectorAll('select') ?? [])];
    expect(selects).toHaveLength(3);
    expect([...(selects[0]?.options ?? [])].map((option) => option.textContent)).toEqual([
      '오전',
      '오후',
    ]);
  });

  it('shows and edits the same moment on the clock of each reader’s account', async () => {
    const moment = at('Asia/Seoul', 2026, 3, 5, 9, 0);
    const seoul = await edit({ from: moment }, SEOUL);
    expect([hour().value, minute().value, period().value]).toEqual(['9', '0', 'am']);
    seoul.unmount();

    // In New York that moment is 19:00 on the 4th.
    const { onChange } = await edit({ from: moment }, NEW_YORK);
    expect(dayCell('2026-03-04')).toHaveAttribute('data-selected');
    expect([hour().value, minute().value, period().value]).toEqual(['7', '0', 'pm']);
    await choose(hour(), 8);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('America/New_York', 2026, 3, 4, 20, 0),
    });
  });

  it('keeps the time of day when another day is picked', async () => {
    const { onChange } = await edit({ from: at('America/New_York', 2026, 3, 5, 14, 30) }, NEW_YORK);
    await pick('2026-03-20');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('America/New_York', 2026, 3, 20, 14, 30),
    });
  });

  it('gives a day picked after turning the time on its nine in the morning', async () => {
    const { onChange } = await edit(undefined, NEW_YORK);
    await fireEvent.click(toggle('Include time'));
    await pick('2026-03-20');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('America/New_York', 2026, 3, 20, 9, 0),
    });
  });

  it('turns a moment back into the day it falls on for the reader', async () => {
    const moment = at('Asia/Seoul', 2026, 3, 5, 9, 0);
    const seoul = await edit({ from: moment }, SEOUL);
    await fireEvent.click(toggle('Include time'));
    await closePicker();
    expect(seoul.onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-05' });
    seoul.unmount();

    const newYork = await edit({ from: moment }, NEW_YORK);
    await fireEvent.click(toggle('Include time'));
    await closePicker();
    expect(newYork.onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-04' });
  });
});

describe('a range with times', () => {
  const morning = at('Asia/Seoul', 2026, 3, 5, 9, 0);
  const evening = at('Asia/Seoul', 2026, 3, 9, 18, 0);

  it('shows a time for each end', async () => {
    await edit({ from: morning, to: evening });
    expect([hour('Start time').value, period('Start time').value]).toEqual(['9', 'am']);
    expect([hour('End time').value, period('End time').value]).toEqual(['6', 'pm']);
    expect(shownText()).toBe('March 05 at 9:00 AM → March 09 at 6:00 PM');
  });

  it('stores two moments', async () => {
    const { onChange } = await edit({ from: morning, to: evening });
    await choose(hour('End time'), 7);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: morning,
      to: at('Asia/Seoul', 2026, 3, 9, 19, 0),
    });
  });

  it('gets nine in the morning on both ends when the time is turned on for a range of days', async () => {
    const { onChange } = await edit({ from: '2026-03-05', to: '2026-03-09' }, NEW_YORK);
    await fireEvent.click(toggle('Include time'));
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('America/New_York', 2026, 3, 5, 9, 0),
      to: at('America/New_York', 2026, 3, 9, 9, 0),
    });
  });

  it('cannot end before it starts: the end pulls the start along', async () => {
    const start = at('Asia/Seoul', 2026, 3, 5, 9, 0);
    const end = at('Asia/Seoul', 2026, 3, 5, 10, 0);
    const { onChange } = await edit({ from: start, to: end });
    await choose(hour('End time'), 8);
    expect([hour('Start time').value, hour('End time').value]).toEqual(['8', '8']);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: at('Asia/Seoul', 2026, 3, 5, 8, 0) });
  });

  it('cannot start after it ends: the start pulls the end along', async () => {
    const start = at('Asia/Seoul', 2026, 3, 5, 9, 0);
    const end = at('Asia/Seoul', 2026, 3, 5, 10, 0);
    const { onChange } = await edit({ from: start, to: end });
    await choose(hour('Start time'), 11);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: at('Asia/Seoul', 2026, 3, 5, 11, 0) });
  });

  it('gives a newly picked end the time of day of the start', async () => {
    const { onChange } = await edit({ from: morning });
    await fireEvent.click(toggle('End date'));
    await pick('2026-03-12');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: morning,
      to: at('Asia/Seoul', 2026, 3, 12, 9, 0),
    });
  });

  it('becomes a range of days on the reader’s calendar when the time is turned off', async () => {
    const { onChange } = await edit({ from: morning, to: evening }, NEW_YORK);
    await fireEvent.click(toggle('Include time'));
    await closePicker();
    // 09:00 on the 5th and 18:00 on the 9th in Seoul are the 4th and the 9th in New York.
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-04', to: '2026-03-09' });
  });
});

describe('times that clocks which change make special', () => {
  it('turns a time the clocks skip into the time it becomes, and shows that', async () => {
    // New York, 8 March 2026: 02:00 to 03:00 does not exist.
    const { onChange } = await edit({ from: at('America/New_York', 2026, 3, 8, 1, 30) }, NEW_YORK);
    await choose(hour(), 2);
    expect(hour()).toHaveValue('3');
    expect(shownText()).toBe('March 08 at 3:30 AM');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('America/New_York', 2026, 3, 8, 3, 30),
    });
  });

  it('shows the stored time again when the skipped time becomes the time it already was', async () => {
    const stored = at('America/New_York', 2026, 3, 8, 3, 30);
    const { onChange } = await edit({ from: stored }, NEW_YORK);
    await choose(hour(), 2);
    expect(hour()).toHaveValue('3');
    await closePicker();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('takes the first of two moments for a time that happens twice', async () => {
    // New York, 1 November 2026: 01:00 to 02:00 happens twice.
    const { onChange } = await edit({ from: at('America/New_York', 2026, 11, 1, 0, 30) }, NEW_YORK);
    await choose(hour(), 1);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: Date.UTC(2026, 10, 1, 5, 30) });
  });

  it('keeps a stored second moment the second while its minute changes', async () => {
    const second = Date.UTC(2026, 10, 1, 6, 30);
    const { onChange } = await edit({ from: second }, NEW_YORK);
    expect([hour().value, minute().value, period().value]).toEqual(['1', '30', 'am']);
    await choose(minute(), 45);
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: second + 15 * 60_000 });
  });

  it('puts a day at nine even on the day the clocks change', async () => {
    const { onChange } = await edit({ from: '2026-03-08' }, NEW_YORK);
    await fireEvent.click(toggle('Include time'));
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      from: at('America/New_York', 2026, 3, 8, 9, 0),
    });
  });
});

describe('the picker of a date property on a narrow screen', () => {
  it('is a sheet with the name of the property, closed by its button, and stores the same value', async () => {
    const { onChange } = await edit(undefined, SEOUL, { presentation: 'sheet' });
    const sheet = screen.getByRole('dialog', { name: 'My date' });
    expect(sheet).toHaveAttribute('data-side', 'bottom');
    await pick('2026-03-15');
    await fireEvent.click(within(sheet).getByRole('button', { name: 'Done' }));
    await tick();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-15' });
  });
});

describe('a date property that may not be edited', () => {
  it('shows the value as text and cannot be opened', () => {
    const { container, onChange } = mount(
      'date',
      { from: '2026-03-05', to: '2026-03-09' },
      { readOnly: true },
    );
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('March 05 → March 09');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is the same in display mode, also for a moment in the reader’s time zone', () => {
    const { container } = mount(
      'date',
      { from: at('Asia/Seoul', 2026, 3, 5, 9, 0) },
      { mode: 'display', regional: NEW_YORK },
    );
    expect(screen.queryByRole('button')).toBeNull();
    expect(container).toHaveTextContent('March 04 at 7:00 PM');
  });

  it('shows the placeholder, or nothing, without a value', async () => {
    const mounted = mount('date', undefined, { readOnly: true });
    expect(mounted.container).toHaveTextContent('Empty');
    await mounted.setProps({ showEmptyPlaceholder: false });
    expect(mounted.container).toHaveTextContent('');
  });

  it('does not open even when asked to start open', () => {
    mount('date', { from: '2026-03-05' }, { readOnly: true, startOpen: true });
    expect(document.querySelector('[data-slot="date-picker"]')).toBeNull();
  });
});

describe('a date property that starts in the editing state', () => {
  it('has its picker open at once and stores what is picked when it closes', async () => {
    const { onChange } = mount('date', { from: '2026-03-05' }, { startOpen: true });
    expect(dayCell('2026-03-05')).toHaveAttribute('data-selected');
    await pick('2026-03-06');
    await closePicker();
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ from: '2026-03-06' });
  });
});
