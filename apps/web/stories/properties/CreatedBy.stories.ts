/** Who created the card: read from the card, never editable. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/CreatedBy',
  component: PropertyStory,
  args: { type: 'createdBy' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = {};

/** A stored value is ignored: the card says who created it. */
export const Empty: Story = { args: { value: '' } };

export const ReadOnly: Story = { args: { readOnly: true } };

/** Asked to start editing, it stays text. */
export const Editing: Story = { args: { startOpen: true } };
