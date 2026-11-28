import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';

import BoardViewSample from './BoardViewSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

/** The cards a view shows in a body: fifty at most. */
const SHOWN = 50;

const meta = {
  title: 'Body/Board view',
  component: BoardViewSample,
  args: { kind: 'table' },
  play: async ({ canvasElement, args }) => {
    const root = await storyElement(canvasElement, '[data-testid="embedded-view"]');
    const count = typeof args.count === 'number' ? args.count : 8;
    await expect(root).toHaveTextContent('Example view');
    await expect(root).toHaveTextContent('Example board with a long name');
    const cards = () => root.querySelectorAll('[data-testid="embedded-card"]').length;
    // A calendar shows the cards still to come when there are any: fewer than it has.
    if (args.kind === 'calendar') {
      await waitFor(() => expect(cards()).toBeGreaterThan(0));
      await expect(cards()).toBeLessThanOrEqual(Math.min(count, SHOWN));
    } else {
      await waitFor(() => expect(cards()).toBe(Math.min(count, SHOWN)));
    }
    const all = root.querySelector('[data-testid="embedded-view-all"]');
    if (count > SHOWN) await expect(all).toHaveTextContent(String(count));
    else await expect(all).toBeNull();
    // What the view shows scrolls within it: the body is never wider for it.
    const body = await storyElement(canvasElement, '[data-testid="board-view-example"]');
    await expect(body.scrollWidth).toBeLessThanOrEqual(body.clientWidth);
  },
} satisfies Meta<typeof BoardViewSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Table: Story = {};
export const Kanban: Story = { args: { kind: 'board' } };
export const Calendar: Story = { args: { kind: 'calendar' } };
export const Gallery: Story = { args: { kind: 'gallery' } };
export const SixtyCards: Story = { args: { count: 60 } };
export const ReadOnly: Story = { args: { readOnly: true } };
/** On a phone's width: the cards' titles stay at the start as the table scrolls within itself. */
export const Phone: Story = {
  args: { narrow: true },
  play: async (context) => {
    await meta.play(context);
    const scroll = await storyElement(
      context.canvasElement,
      '[data-testid="embedded-view-scroll"]',
    );
    const first = scroll.querySelector('tbody .embedded-first');
    await expect(first && getComputedStyle(first).position).toBe('sticky');
  },
};
/** A card pressed opens over the page. */
export const OpenCard: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="embedded-card"][href$="example-card-1"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="opened-card"]')).toHaveTextContent(
        'example-card-1',
      ),
    );
  },
};
/** A view that is not there, as one the reader may not see: the same one line. */
export const Unseen: Story = {
  args: { missing: true },
  play: async ({ canvasElement }) => {
    const said = await storyElement(canvasElement, '[data-testid="board-view-unseen"]');
    await expect(said).toBeVisible();
    await expect(canvasElement.querySelector('[data-testid="embedded-view"]')).toBeNull();
  },
};
