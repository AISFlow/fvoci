/** Who last changed the card: read from the card, never editable. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/UpdatedBy',
  component: PropertyStory,
  args: { type: 'updatedBy' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = {};

/** A stored value is ignored: the card says who changed it. */
export const Empty: Story = { args: { value: '' } };

export const ReadOnly: Story = { args: { readOnly: true } };

/** Asked to start editing, it stays text. */
export const Editing: Story = { args: { startOpen: true } };
