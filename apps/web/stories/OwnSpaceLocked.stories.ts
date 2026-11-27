import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, fn, userEvent, waitFor } from 'storybook/test';
import OwnSpaceLocked from '$lib/features/account/OwnSpaceLocked.svelte';
import { m } from '$lib/i18n/paraglide/messages.js';
import { BackendError } from '$lib/state/index.ts';
import { storyElement } from './interactions.ts';

const meta = {
  title: 'Account/Own space locked',
  component: OwnSpaceLocked,
  args: { username: 'example-reader', onConfirm: fn(() => new Promise<void>(() => undefined)) },
} satisfies Meta<typeof OwnSpaceLocked>;
export default meta;
type Story = StoryObj<typeof meta>;

async function give(canvas: HTMLElement): Promise<void> {
  const field = await storyElement(canvas, '[data-testid="own-space-locked-password"]');
  await userEvent.type(field, 'a long password');
  await userEvent.keyboard('{Enter}');
}

/** A provider's session in the own space of an account with a password: why, and the password. */
export const Asking: Story = {
  play: async ({ canvasElement, args }) => {
    const shown = await storyElement(canvasElement, '[data-testid="own-space-locked"]');
    await expect(shown).toHaveTextContent(m.own_space_locked_text());
    await give(canvasElement);
    await expect(args.onConfirm).toHaveBeenCalledWith('a long password');
  },
};

/** A wrong password: said at the field. */
export const WrongPassword: Story = {
  args: {
    onConfirm: fn(() =>
      Promise.reject(new BackendError(400, { reason: 'invalid_credentials', detail: 'wrong' })),
    ),
  },
  play: async ({ canvasElement }) => {
    await give(canvasElement);
    const problem = await storyElement(canvasElement, '[data-testid="own-space-locked-problem"]');
    await waitFor(() => expect(problem).toHaveTextContent(m.account_password_wrong()));
  },
};
