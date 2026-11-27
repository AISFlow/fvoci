/**
 * One option out of a list that can be searched and added to. The button next
 * to an option opens its name, colour and Delete.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Select',
  component: PropertyStory,
  args: { type: 'select' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: 'option-2' } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: 'option-3', readOnly: true } };

/** Starts with the list open. */
export const Editing: Story = { args: { value: 'option-2', startOpen: true } };

/** Sixty options: the list scrolls and typing narrows it. */
export const ManyOptions: Story = { args: { value: 'option-7', optionCount: 60 } };

/** A property without options invites to type the first one. */
export const NoOptionsYet: Story = { args: { optionCount: 0, startOpen: true } };
