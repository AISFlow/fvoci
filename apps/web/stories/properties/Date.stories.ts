/**
 * A day, a moment, or a range. A day is the same for every reader; a moment is
 * entered and shown on the clock of the account time zone given in the
 * arguments, whatever the time zone of this device.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Date',
  component: PropertyStory,
  args: { type: 'date' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

/** A day without a time. */
export const Day: Story = { args: { value: { from: '2026-11-12' } } };

/** A range of days, both ends included. */
export const Range: Story = { args: { value: { from: '2026-11-12', to: '2026-11-20' } } };

/** A moment: 15:30 on 12 November in Seoul. */
export const WithTime: Story = { args: { value: { from: Date.UTC(2026, 10, 12, 6, 30) } } };

/** From one moment to another. */
export const RangeWithTime: Story = {
  args: { value: { from: Date.UTC(2026, 10, 12, 0, 0), to: Date.UTC(2026, 10, 13, 9, 0) } },
};

/**
 * The moment of `WithTime` for a reader whose account is set to New York:
 * 01:30 the same day there. It stays that way on a device in Seoul.
 */
export const AnotherTimeZone: Story = {
  args: { value: { from: Date.UTC(2026, 10, 12, 6, 30) }, timeZone: 'America/New_York' },
};

/** A reader in Berlin: hours to 23, weeks from Monday. */
export const TwentyFourHoursMondayFirst: Story = {
  args: {
    value: { from: Date.UTC(2026, 10, 12, 6, 30) },
    timeZone: 'Europe/Berlin',
    hourCycle: 'h23',
    weekStart: 1,
  },
};

/**
 * New York on the night the clocks went back: 01:30 happened twice. This is
 * the second one.
 */
export const ClocksChange: Story = {
  args: { value: { from: Date.UTC(2026, 10, 1, 6, 30) }, timeZone: 'America/New_York' },
};

export const Empty: Story = {};

export const ReadOnly: Story = {
  args: { value: { from: '2026-11-12', to: '2026-11-20' }, readOnly: true },
};

/** Starts with the picker open. */
export const Editing: Story = { args: { value: { from: '2026-11-12' }, startOpen: true } };
