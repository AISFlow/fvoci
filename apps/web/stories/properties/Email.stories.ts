/** One email address, checked for its shape before it is saved. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Email',
  component: PropertyStory,
  args: { type: 'email' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: 'example-reader@example.test' } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: 'example-guest@example.test', readOnly: true } };

export const Editing: Story = { args: { value: 'example-reader@example.test', startOpen: true } };
