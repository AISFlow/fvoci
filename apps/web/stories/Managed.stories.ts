import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';
import ManagedSample from './ManagedSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Workspace/Managed board',
  component: ManagedSample,
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="board-managed"]');
  },
} satisfies Meta<typeof ManagedSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const NotAMember: Story = {};
export const JoinAsViewer: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="board-managed"] button');
    await expect(
      await storyElement(canvasElement, '[data-testid="joined-board"]'),
    ).toHaveTextContent('viewer');
  },
};
export const JoinRefused: Story = {
  args: { failJoin: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="board-managed"] button');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="managed-problems"]')).toHaveTextContent(
        '1',
      ),
    );
    await storyElement(canvasElement, '[data-testid="board-managed"]');
  },
};
