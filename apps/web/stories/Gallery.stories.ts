import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';
import ViewSample from './ViewSample.svelte';
import { clickStory, selectTwo, storyElement } from './interactions.ts';

const meta = {
  title: 'Views/Gallery',
  component: ViewSample,
  args: { kind: 'gallery' },
  play: async ({ canvasElement, args }) => {
    const root = await storyElement(canvasElement, '[data-testid="gallery"]');
    const count = typeof args.count === 'number' ? args.count : 8;
    await waitFor(() => expect(root.querySelectorAll('[data-card]')).toHaveLength(count));
    if (count > 0) await expect(root).toHaveTextContent('Example card 001');
    if (args.readOnly)
      await expect(root.querySelectorAll('[data-testid="gallery-new"]')).toHaveLength(0);
    else await expect(root.querySelector('[data-testid="gallery-new"]')).not.toBeNull();
  },
} satisfies Meta<typeof ViewSample>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Cards: Story = {};
export const Empty: Story = { args: { count: 0 } };
export const ThreeHundredCards: Story = { args: { count: 300 } };
export const ReadOnly: Story = { args: { readOnly: true } };

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
export const SelectedNarrow: Story = {
  args: { narrow: true },
  play: Selected.play,
};
