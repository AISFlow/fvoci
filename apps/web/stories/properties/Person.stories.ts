/**
 * One person, picked from the people of the board. Typing narrows the list by
 * login name and display name.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Person',
  component: PropertyStory,
  args: { type: 'person' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: 'example-member-1' } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: 'example-member-3', readOnly: true } };

/** Starts with the list open. */
export const Editing: Story = { args: { value: 'example-member-2', startOpen: true } };

/** Someone who is no longer among the people of the board. */
export const UnknownUser: Story = { args: { value: 'u-gone' } };

/** Somebody without a display name: the login name alone. */
export const LoginNameOnly: Story = { args: { value: 'example-member-6' } };
