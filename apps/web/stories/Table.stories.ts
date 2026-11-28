import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';
import ViewSample from './ViewSample.svelte';
import { clickStory, selectTwo, storyElement } from './interactions.ts';

const meta = {
  title: 'Views/Table',
  component: ViewSample,
  args: { kind: 'table' },
  play: async ({ canvasElement, args }) => {
    const root = await storyElement(canvasElement, '[data-testid="table-view"]');
    const count = typeof args.count === 'number' ? args.count : 8;
    await waitFor(() => expect(root.querySelectorAll('[data-card]')).toHaveLength(count));
    if (count > 0) await expect(root).toHaveTextContent('Example card 001');
    if (args.readOnly) await expect(root.querySelectorAll('[data-kind="add"]')).toHaveLength(0);
    else await expect(root.querySelector('[data-kind="add"]')).not.toBeNull();
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
    (await storyElement(canvasElement, '[data-row="example-card-1"] [data-kind="title"]')).focus();
    await clickStory(canvasElement, '[data-more="example-card-1"]');
    await clickStory(canvasElement, '[data-action="open"]');
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
      first: '[data-row="example-card-1"] [data-kind="title"]',
      menu: '[data-more="example-card-1"]',
      second: '[data-row="example-card-2"] [data-kind="title"]',
    });
  },
};
export const SelectedNarrow: Story = {
  args: { narrow: true },
  play: Selected.play,
};
/** A whom, a status and a due date: three buttons beside the menu and ✕, on the narrowest phone. */
export const SelectedNarrowest: Story = {
  args: { quick: true, width: 320 },
  play: Selected.play,
};
/** Just wide enough for the words, which give way to keep ✕ in view. */
export const SelectedMiddle: Story = {
  args: { quick: true, width: 720 },
  play: Selected.play,
};
