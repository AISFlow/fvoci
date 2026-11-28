import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, waitFor, within } from 'storybook/test';

import { m } from '$lib/i18n/paraglide/messages.js';

import ReactionsSample from './ReactionsSample.svelte';
import { storyElement } from './interactions.ts';

/** The reactions to a card and to its comments (features/card/CommentReactions). */
const meta = {
  title: 'Card/Reactions',
  component: ReactionsSample,
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="reactions"]');
  },
} satisfies Meta<typeof ReactionsSample>;
export default meta;
type Story = StoryObj<typeof meta>;

/** The reader's own pressed; pressed again it comes off; one of the quick row goes on. */
export const OwnAndOthers: Story = {
  play: async ({ canvasElement }) => {
    const comment = await storyElement(canvasElement, '[data-comment] [data-testid="reactions"]');
    const group = within(comment);
    const thumbs = group.getByRole('button', { name: /^👍 2/ });
    await expect(thumbs).toHaveAttribute('aria-pressed', 'true');
    await userEvent.click(thumbs);
    await waitFor(() =>
      expect(group.getByRole('button', { name: /^👍 1/ })).toHaveAttribute('aria-pressed', 'false'),
    );
    // The comment's own button: `storyElement` looks in the whole document, the card's comes first.
    const add = comment.querySelector<HTMLElement>('[data-action="react"]');
    if (!add) throw new Error('the comment has no button to react with');
    await userEvent.click(add);
    const quick = await within(canvasElement.ownerDocument.body).findByRole('group', {
      name: m.reaction_quick(),
    });
    await userEvent.click(within(quick).getByRole('button', { name: '😄' }));
    await waitFor(() =>
      expect(group.getByRole('button', { name: /^😄 1/ })).toHaveAttribute('aria-pressed', 'true'),
    );
  },
};

/** Somebody the reader is not told of is "someone". */
export const Someone: Story = {
  play: async ({ canvasElement }) => {
    const eyes = await storyElement(canvasElement, '[data-reaction="👀"]');
    await expect(eyes.getAttribute('aria-label')).toContain(m.reaction_someone());
  },
};

/** A viewer sees them and presses none, and has nothing to add them with. */
export const ReadOnly: Story = {
  args: { readOnly: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-reaction="🚀"]');
    await expect(
      canvasElement.querySelector('[data-reaction] button, button[data-reaction]'),
    ).toBeNull();
    await expect(canvasElement.querySelector('[data-action="react"]')).toBeNull();
  },
};

/** Twenty different emoji on the card: only those already there are offered. */
export const Full: Story = {
  args: { full: true },
  play: async ({ canvasElement }) => {
    const card = await storyElement(
      canvasElement,
      '[data-testid="reactions-example"] > [data-testid="reactions"]',
    );
    await userEvent.click(within(card).getByRole('button', { name: m.reaction_add() }));
    const body = within(canvasElement.ownerDocument.body);
    await body.findByText(m.reaction_full());
    const quick = await body.findByRole('group', { name: m.reaction_quick() });
    await expect(within(quick).getByRole('button', { name: '👍' })).toBeDisabled();
    await expect(within(quick).getByRole('button', { name: '🚀' })).toBeEnabled();
  },
};
