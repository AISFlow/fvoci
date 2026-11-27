/**
 * One line of text, edited in place: Enter or leaving saves, Escape puts the
 * value back.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Text',
  component: PropertyStory,
  args: { type: 'text' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: '회의록 정리하기 — write up the notes' } };

export const Empty: Story = {};

export const ReadOnly: Story = { args: { value: '읽기 전용 값 Read-only value', readOnly: true } };

/** Starts with the field focused. */
export const Editing: Story = { args: { value: '고치는 중', startOpen: true } };
