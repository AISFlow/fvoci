/**
 * When the card was created: read from the card, shown on the clock of the
 * account, never editable.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/CreatedTime',
  component: PropertyStory,
  args: { type: 'createdTime' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

/** 21:30 on 9 November in Seoul. */
export const WithValue: Story = {};

/** A card always has a created time; a stored value is ignored. */
export const Empty: Story = { args: { value: '' } };

export const ReadOnly: Story = { args: { readOnly: true } };

/** Asked to start editing, it stays text. */
export const Editing: Story = { args: { startOpen: true } };

/** The same moment for a reader whose account is set to New York, hours to 23. */
export const AnotherTimeZone: Story = { args: { timeZone: 'America/New_York', hourCycle: 'h23' } };
