import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, waitFor } from 'storybook/test';
import { m } from '$lib/i18n/paraglide/messages.js';

import CourseSample from './CourseSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

async function ensureTreeOpen(canvasElement: HTMLElement): Promise<void> {
  await storyElement(canvasElement, '[data-testid="course-screen"]');
  const opener = canvasElement.querySelector<HTMLElement>('[data-testid="open-unit-tree"]');
  if (opener && opener.getAttribute('aria-expanded') !== 'true') {
    await userEvent.click(opener);
  }
}

const meta = {
  title: 'Course/Units',
  component: CourseSample,
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="course-screen"]');
  },
} satisfies Meta<typeof CourseSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const ChaptersAndUnits: Story = {
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="unit-view"]');
    await storyElement(canvasElement, '[role="treeitem"][aria-level="2"]');
  },
};

export const Narrow390: Story = {
  args: { narrow: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="unit-view"]');
    await clickStory(canvasElement, '[data-testid="open-unit-tree"]');
    await storyElement(canvasElement, '[data-testid="unit-tree"]');
  },
};

export const EmptyCourse: Story = {
  args: { empty: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="course-first-unit"]');
  },
};

export const ReadOnly: Story = {
  args: { readOnly: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, 'h1[data-testid="unit-title"]');
    await expect(canvasElement.querySelector('[data-testid="new-unit"]')).toBeNull();
  },
};

export const ManagersMenu: Story = {
  args: { manager: true },
  play: async ({ canvasElement }) => {
    await ensureTreeOpen(canvasElement);
    await clickStory(canvasElement, '[data-testid="course-menu"]');
    await storyElement(canvasElement, '[data-action="edit-course"]');
  },
};

export const KeyboardInTheTree: Story = {
  play: async ({ canvasElement }) => {
    await ensureTreeOpen(canvasElement);
    const first = await storyElement(canvasElement, '[role="treeitem"][tabindex="0"]');
    first.focus();
    await userEvent.keyboard('{Home}');
    await waitFor(() =>
      expect(canvasElement.ownerDocument.activeElement).toHaveAttribute('aria-expanded', 'true'),
    );
    await userEvent.keyboard('{ArrowLeft}');
    await waitFor(() =>
      expect(canvasElement.ownerDocument.activeElement).toHaveAttribute('aria-expanded', 'false'),
    );
  },
};

export const Outline: Story = {
  args: { outlined: true },
  play: async ({ canvasElement }) => {
    const shown = await storyElement(
      canvasElement,
      'nav[data-testid="page-outline"], [data-testid="open-page-outline"]',
    );
    const opener = shown.dataset.testid === 'open-page-outline' ? shown : undefined;
    if (opener) await userEvent.click(opener);
    const nav = await storyElement(canvasElement, 'nav[data-testid="page-outline"]');
    await expect(nav).toHaveAccessibleName(m.wiki_outline());
    const items = nav.querySelectorAll<HTMLElement>('li button');
    await expect(items).toHaveLength(8);
    const chosen = items[3];
    if (!chosen) throw new Error('no fourth heading in the outline');
    await userEvent.click(chosen);
    const heading = Array.from(canvasElement.querySelectorAll('.doc-body h3')).find((one) =>
      one.textContent.startsWith('Reading units'),
    );
    await waitFor(() => expect(canvasElement.ownerDocument.activeElement).toBe(heading));
  },
};
