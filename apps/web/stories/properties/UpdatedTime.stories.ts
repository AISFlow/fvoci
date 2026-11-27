/**
 * When the card was last changed: read from the card, shown on the clock of
 * the account, never editable.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/UpdatedTime',
  component: PropertyStory,
  args: { type: 'updatedTime' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

/** 01:05 on 11 November in Seoul. */
export const WithValue: Story = {};

/** A card always has an updated time; a stored value is ignored. */
export const Empty: Story = { args: { value: '' } };

export const ReadOnly: Story = { args: { readOnly: true } };

/** Asked to start editing, it stays text. */
export const Editing: Story = { args: { startOpen: true } };

/**
 * The same moment for a reader whose account is set to New York: still the
 * 10th there.
 */
export const AnotherTimeZone: Story = { args: { timeZone: 'America/New_York' } };
