/**
 * What was done to a card, the newest first, in two rows a line: who and
 * when, then what. Folded until it is opened; on a phone its header is a
 * 44px button and no line grows past its two rows.
 */
import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';

import CardActivitySample from './CardActivitySample.svelte';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Cards/Activity',
  component: CardActivitySample,
  play: async ({ canvasElement }) => {
    const header = await storyElement(canvasElement, '[data-testid="card-activity"] button');
    await expect(header).toHaveAttribute('aria-expanded', 'false');
    await clickStory(canvasElement, '[data-testid="card-activity"] button');
    const lines = await storyElement(canvasElement, '[data-testid="card-activity"] ol');
    await waitFor(() => expect(lines.querySelectorAll('li')).toHaveLength(4));
    await expect(lines.querySelector('[data-slot="agent-badge"]')).not.toBeNull();
  },
} satisfies Meta<typeof CardActivitySample>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Opened: Story = {};

/** At 390px: the header button is 44px tall, and each line keeps to two rows. */
export const Phone: Story = {
  args: { narrow: true },
  play: async (context) => {
    await meta.play(context);
    const { canvasElement } = context;
    const header = await storyElement(canvasElement, '[data-testid="card-activity"] button');
    await expect(header.getBoundingClientRect().height).toBeGreaterThanOrEqual(44);
    for (const item of canvasElement.querySelectorAll('[data-testid="card-activity"] li')) {
      const rows = [...item.children].map((row) => row.getBoundingClientRect());
      await expect(rows).toHaveLength(2);
      const first = rows[0];
      if (first) await expect(first.height).toBeLessThan(32);
    }
  },
};
