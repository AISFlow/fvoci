import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, waitFor, within } from 'storybook/test';

import { RELATION_LIMIT } from '$lib/logic/wire.ts';

import RelationSample from './RelationSample.svelte';

const meta = {
  title: 'Properties/Relation',
  component: RelationSample,
} satisfies Meta<typeof RelationSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const WithValue: Story = {};
export const Empty: Story = { args: { selected: 0 } };
export const EmptyBoard: Story = { args: { count: 0, selected: 0, startOpen: true } };
export const Editing: Story = { args: { startOpen: true } };
export const ReadOnly: Story = { args: { readOnly: true } };
export const FiveHundredCandidates: Story = {
  args: { count: 500, startOpen: true },
  play: async ({ canvasElement }) => {
    const body = canvasElement.ownerDocument.body;
    await waitFor(() => expect(body.querySelector('[data-slot="relation-more"]')).not.toBeNull());
    await expect(body.querySelectorAll('[role="option"]')).toHaveLength(50);
  },
};
export const ExistingAndPreventedCycles: Story = { args: { loop: true, startOpen: true } };
export const DeletedCardsFillTheLimit: Story = {
  args: { selected: 0, hidden: RELATION_LIMIT, startOpen: true },
  play: async ({ canvasElement }) => {
    await waitFor(async () => {
      const body = canvasElement.ownerDocument.body;
      await expect(body.querySelector('[data-slot="relation-held-back"]')).toHaveTextContent(
        String(RELATION_LIMIT),
      );
      await expect(body.querySelectorAll('[role="option"]')).toHaveLength(0);
    });
  },
};
export const ReleaseDeletedCards: Story = {
  ...DeletedCardsFillTheLimit,
  play: async ({ canvasElement }) => {
    const body = canvasElement.ownerDocument.body;
    await waitFor(() =>
      expect(body.querySelector('[data-slot="relation-drop-hidden"]')).not.toBeNull(),
    );
    const button = body.querySelector<HTMLButtonElement>('[data-slot="relation-drop-hidden"]');
    if (!button) throw new Error('Missing release button');
    await userEvent.click(button);
    await waitFor(() =>
      expect(within(canvasElement).getByTestId('relation-value')).toHaveTextContent('[]'),
    );
    await waitFor(() => expect(body.querySelectorAll('[role="option"]').length).toBeGreaterThan(0));
  },
};
