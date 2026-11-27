/**
 * A number, stored as the text that was typed. What does not start with a
 * number is not saved.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Number',
  component: PropertyStory,
  args: { type: 'number' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: '12.5' } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: '42', readOnly: true } };

/** Starts with the field focused. Type letters to see a value that is refused. */
export const Editing: Story = { args: { value: '3', startOpen: true } };
