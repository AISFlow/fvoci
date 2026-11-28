import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';
import { WINDOW_FROM } from '$lib/features/views/kanban/column-window.ts';
import ViewSample from './ViewSample.svelte';
import { clickStory, selectTwo, storyElement } from './interactions.ts';

const meta = {
  title: 'Views/Kanban',
  component: ViewSample,
  args: { kind: 'board' },
  play: async ({ canvasElement, args }) => {
    const root = await storyElement(canvasElement, '[data-testid="kanban"]');
    const count = typeof args.count === 'number' ? args.count : 8;
    // Every card, as the columns count them: a long column draws only a window of its cards.
    const counted = (): number =>
      [...root.querySelectorAll('[data-column-count] [aria-hidden="true"]')].reduce(
        (sum, each) => sum + Number(each.textContent),
        0,
      );
    await waitFor(() => expect(counted()).toBe(count));
    // Each column draws every card it counts, or a window of them once it is long.
    for (const column of root.querySelectorAll('[data-column]')) {
      const cards = Number(
        column.querySelector('[data-column-count] [aria-hidden="true"]')?.textContent,
      );
      const drawn = column.querySelectorAll('[data-card]').length;
      if (cards <= WINDOW_FROM) await expect(drawn).toBe(cards);
      else await expect(drawn).toBeGreaterThan(0);
    }
    if (count > 0) await expect(root).toHaveTextContent('Example card 001');
    if (args.readOnly)
      await expect(root.querySelectorAll('[data-column-add], [data-column-new]')).toHaveLength(0);
    else await expect(root.querySelector('[data-column-add], [data-column-new]')).not.toBeNull();
  },
} satisfies Meta<typeof ViewSample>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Cards: Story = {};
export const Empty: Story = { args: { count: 0 } };
export const ThreeHundredCards: Story = { args: { count: 300 } };
export const ReadOnly: Story = { args: { readOnly: true } };
/** 600 cards, 400 of them in one column, on a phone's width: the column draws a window of them. */
export const LongColumn: Story = {
  args: { count: 600, narrow: true },
  play: async (context) => {
    await meta.play(context);
    const column = await storyElement(context.canvasElement, '[data-column-cards="todo"]');
    const drawn = column.querySelectorAll('[data-card]').length;
    await expect(drawn).toBeGreaterThan(0);
    await expect(drawn).toBeLessThan(100);
    await expect(column.querySelector('[role="listitem"]')?.getAttribute('aria-setsize')).toBe(
      '400',
    );
  },
};

export const OpenCard: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-card="example-card-1"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="opened-card"]')).toHaveTextContent(
        'example-card-1',
      ),
    );
  },
};

export const Selected: Story = {
  play: async ({ canvasElement }) => {
    await selectTwo(canvasElement, {
      first: '[data-card="example-card-1"]',
      menu: '[data-card-menu="example-card-1"]',
      second: '[data-card="example-card-2"]',
    });
  },
};
