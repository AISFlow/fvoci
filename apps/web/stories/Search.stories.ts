import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';

import { m } from '$lib/i18n/paraglide/messages.js';

import { storyElement } from './interactions.ts';
import SearchSample from './SearchSample.svelte';

const meta = { title: 'Workspace/Search', component: SearchSample } satisfies Meta<
  typeof SearchSample
>;
export default meta;
type Story = StoryObj<typeof meta>;

/** The dialog once its search has settled in `status`. */
async function settled(canvas: HTMLElement, status: string): Promise<HTMLElement> {
  const dialog = await storyElement(canvas, '[data-testid="search-dialog"]');
  await waitFor(() => expect(dialog).toHaveAttribute('data-status', status));
  return dialog;
}

export const Found: Story = {
  play: async ({ canvasElement }) => {
    const dialog = await settled(canvasElement, 'found');
    const options = dialog.querySelectorAll('[role="option"]');
    await expect(options).toHaveLength(3);
    await expect(options[0]).toHaveAttribute('aria-selected', 'true');
    await expect(dialog.querySelectorAll('mark')).toHaveLength(2);
    await expect(dialog.querySelector('[data-hit-snippet] script')).toBeNull();
    // A page without a title, and a board the sidebar does not know: no name.
    await expect(options[1]).toHaveTextContent(m.search_untitled_page());
    await expect(options[2]?.querySelector('[data-hit-container]')).toBeNull();
  },
};

export const Nothing: Story = {
  args: { answer: 'none' },
  play: async ({ canvasElement }) => {
    const dialog = await settled(canvasElement, 'none');
    await expect(dialog).toHaveTextContent(m.search_none());
  },
};

export const Unavailable: Story = {
  args: { answer: 'unavailable' },
  play: async ({ canvasElement }) => {
    const dialog = await settled(canvasElement, 'unavailable');
    await expect(dialog).toHaveTextContent(m.search_unavailable());
    await expect(dialog.querySelector('[data-testid="search-retry"]')).not.toBeNull();
  },
};

export const NothingTyped: Story = {
  args: { answer: 'empty' },
  play: async ({ canvasElement }) => {
    const dialog = await settled(canvasElement, 'empty');
    await expect(dialog).toHaveTextContent(m.search_hint());
  },
};
