/** Pressing it changes it at once. `true` or nothing is stored. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Checkbox',
  component: PropertyStory,
  args: { type: 'checkbox' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Checked. */
export const WithValue: Story = { args: { value: 'true' } };

/** Unchecked: there is no other empty state. */
export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: 'true', readOnly: true } };

/** A checkbox has no editing state of its own; it is the same box. */
export const Editing: Story = { args: { value: '', startOpen: true } };
