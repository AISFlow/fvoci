import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';
import PeopleSample from './PeopleSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Workspace/Guest invitation',
  component: PeopleSample,
  args: { guest: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="share-guest"]');
  },
} satisfies Meta<typeof PeopleSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const BeforeInviting: Story = {
  play: async ({ canvasElement }) => {
    // The links out for the board: one made here, one mailed to somebody.
    await expect(
      await storyElement(canvasElement, '[data-testid="share-guest-links"]'),
    ).toHaveTextContent('guest@example.test');
  },
};
export const LinkCreated: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="share-guest-make"]');
    await expect(await storyElement(canvasElement, '[data-testid="share-guest-link"]')).toHaveValue(
      'https://example.test/invite/example-invitation-3',
    );
  },
};
export const Working: Story = {
  args: { mode: 'loading' },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="share-guest-make"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="share-guest-make"]')).toBeDisabled(),
    );
  },
};
export const NotAllowed: Story = {
  args: { mode: 'forbidden' },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="share-guest-make"]');
    await storyElement(canvasElement, '[role="alert"]');
  },
};
export const WriteFailed: Story = { ...NotAllowed, args: { mode: 'write-error' } };
