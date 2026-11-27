/** Several options. The list stays open while options are added and taken out. */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/MultiSelect',
  component: PropertyStory,
  args: { type: 'multiSelect' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: ['option-1', 'option-3'] } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: ['option-2', 'option-4'], readOnly: true } };

/** Starts with the list open. */
export const Editing: Story = { args: { value: ['option-1'], startOpen: true } };

/** Sixty options, a dozen of them selected. */
export const ManyOptions: Story = {
  args: {
    value: Array.from({ length: 12 }, (_, index) => `option-${index * 3 + 1}`),
    optionCount: 60,
  },
};
