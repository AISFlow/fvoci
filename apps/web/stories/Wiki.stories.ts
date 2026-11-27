import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, waitFor } from 'storybook/test';
import { m } from '$lib/i18n/paraglide/messages.js';

import WikiSample from './WikiSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Wiki/Pages',
  component: WikiSample,
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="wiki-screen"]');
  },
} satisfies Meta<typeof WikiSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const TreeAndPage: Story = {
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="page-view"]');
    await storyElement(canvasElement, '[role="treeitem"][aria-level="3"]');
  },
};
export const NoPages: Story = {
  args: { empty: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="wiki-first-page"]');
  },
};
export const ReadOnly: Story = {
  args: { readOnly: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, 'h1[data-testid="page-title"]');
    await expect(canvasElement.querySelector('[data-testid="new-page"]')).toBeNull();
  },
};
export const ManagersMenu: Story = {
  args: { manager: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="wiki-menu"]');
    await storyElement(canvasElement, '[data-action="edit-wiki"]');
  },
};
export const KeyboardInTheTree: Story = {
  play: async ({ canvasElement }) => {
    const first = await storyElement(canvasElement, '[role="treeitem"][tabindex="0"]');
    first.focus();
    await userEvent.keyboard('{End}');
    await waitFor(() =>
      expect(canvasElement.ownerDocument.activeElement).toHaveAttribute('aria-posinset', '3'),
    );
    await userEvent.keyboard('{Home}{ArrowLeft}');
    await waitFor(() =>
      expect(canvasElement.ownerDocument.activeElement).toHaveAttribute('aria-expanded', 'false'),
    );
  },
};
export const Outline: Story = {
  args: { outlined: true },
  play: async ({ canvasElement }) => {
    // Beside the page where the column has room, behind its button where it has not.
    const shown = await storyElement(
      canvasElement,
      'nav[data-testid="page-outline"], [data-testid="open-page-outline"]',
    );
    const opener = shown.dataset.testid === 'open-page-outline' ? shown : undefined;
    if (opener) await userEvent.click(opener);
    const nav = await storyElement(canvasElement, 'nav[data-testid="page-outline"]');
    await expect(nav).toHaveAccessibleName(m.wiki_outline());
    const items = nav.querySelectorAll<HTMLElement>('li button');
    await expect(items).toHaveLength(10);
    const chosen = items[3];
    if (!chosen) throw new Error('no fourth heading in the outline');
    await userEvent.click(chosen);
    const heading = Array.from(canvasElement.querySelectorAll('.doc-body h3')).find((one) =>
      one.textContent.startsWith('Two steps'),
    );
    await waitFor(() => expect(canvasElement.ownerDocument.activeElement).toBe(heading));
    if (!opener) await expect(chosen).toHaveAttribute('aria-current', 'location');
  },
};
