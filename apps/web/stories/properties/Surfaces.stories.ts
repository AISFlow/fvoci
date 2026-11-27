/**
 * The two surfaces a picker opens on. Which one is used is one argument of
 * the property components (`presentation`); here it is fixed per story so
 * that both can be seen on any screen.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Surfaces',
  component: PropertyStory,
  args: { type: 'select' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The wide-screen surface: a popover next to the value. */
export const SelectOnPopover: Story = {
  args: { type: 'select', value: 'option-2', presentation: 'popover', startOpen: true },
};

/**
 * The narrow-screen surface: a sheet from the bottom edge. Rows are at least
 * as tall as a finger needs.
 */
export const SelectOnSheet: Story = {
  args: { type: 'select', value: 'option-2', presentation: 'sheet', startOpen: true },
};

export const MultiSelectOnSheet: Story = {
  args: {
    type: 'multiSelect',
    value: ['option-1', 'option-3'],
    optionCount: 30,
    presentation: 'sheet',
    startOpen: true,
  },
};

export const PersonOnPopover: Story = {
  args: { type: 'person', value: 'example-member-1', presentation: 'popover', startOpen: true },
};

export const PersonOnSheet: Story = {
  args: {
    type: 'multiPerson',
    value: ['example-member-1'],
    presentation: 'sheet',
    startOpen: true,
  },
};

export const DateOnPopover: Story = {
  args: {
    type: 'date',
    value: { from: '2026-11-12', to: '2026-11-20' },
    presentation: 'popover',
    startOpen: true,
  },
};

/** The calendar grows to cells a finger can hit. */
export const DateOnSheet: Story = {
  args: {
    type: 'date',
    value: { from: Date.UTC(2026, 10, 12, 6, 30) },
    presentation: 'sheet',
    startOpen: true,
  },
};

/**
 * The default a caller gets from `PickerPresentation`: a sheet on a compact page (under 600px), a
 * popover from there on. Resize the window, then open the value.
 */
export const FollowsTheViewport: Story = {
  args: { type: 'select', value: 'option-1', presentation: 'auto' },
};
