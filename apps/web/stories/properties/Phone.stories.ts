/** A phone number, saved as typed. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Phone',
  component: PropertyStory,
  args: { type: 'phone' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: '010-1234-5678' } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: '+82 2-123-4567', readOnly: true } };

export const Editing: Story = { args: { value: '010-1234-5678', startOpen: true } };
