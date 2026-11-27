/** Several people. The list stays open while people are added and taken out. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/MultiPerson',
  component: PropertyStory,
  args: { type: 'multiPerson' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: ['example-member-1', 'example-member-3'] } };

export const Empty: Story = {};

export const ReadOnly: Story = {
  args: { value: ['example-member-2', 'example-member-4'], readOnly: true },
};

/** Starts with the list open. */
export const Editing: Story = { args: { value: ['example-member-1'], startOpen: true } };

/** One of the people is no longer on the board; they can be taken out. */
export const UnknownUser: Story = { args: { value: ['example-member-1', 'u-gone'] } };
