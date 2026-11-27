/**
 * An address: a link with buttons to edit and copy it, or a field while there
 * is none.
 */
import type { Meta, StoryObj } from '@storybook/svelte';

import PropertyStory from './PropertyStory.svelte';

const meta = {
  title: 'Properties/Url',
  component: PropertyStory,
  args: { type: 'url' },
} satisfies Meta<typeof PropertyStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = { args: { value: 'https://example.test/boards/한글-주소' } };

/** Without a value it is a field at once. */
export const Empty: Story = {};

/** The link and the Copy button stay; the Edit button is gone. */
export const ReadOnly: Story = { args: { value: 'example.test/readonly', readOnly: true } };

/** Starts as a focused field holding the address. */
export const Editing: Story = { args: { value: 'https://example.test', startOpen: true } };

/** An address whose scheme is not allowed is shown as text. */
export const NotALink: Story = { args: { value: 'ftp://example.test/file' } };
