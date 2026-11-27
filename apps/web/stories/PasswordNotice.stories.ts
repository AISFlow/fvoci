import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, fn, userEvent } from 'storybook/test';
import PasswordNotice from '$lib/features/account/PasswordNotice.svelte';
import { m } from '$lib/i18n/paraglide/messages.js';
import { BackendError } from '$lib/state/index.ts';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Account/Password notice',
  component: PasswordNotice,
  args: {
    username: 'example-reader',
    onSet: fn(() => Promise.resolve()),
    onSignInAgain: fn(() => Promise.resolve()),
    onDismiss: fn(),
    onClose: fn(),
  },
} satisfies Meta<typeof PasswordNotice>;
export default meta;
type Story = StoryObj<typeof meta>;

async function setPassword(canvas: HTMLElement): Promise<void> {
  const field = await storyElement(canvas, '[data-testid="password-notice-field"]');
  await userEvent.type(field, 'a long password');
  await userEvent.keyboard('{Enter}');
}

/** An account without a password, in its own space: why to set one, and the form. */
export const Asking: Story = {
  play: async ({ canvasElement }) => {
    const notice = await storyElement(canvasElement, '[data-testid="password-notice"]');
    await expect(notice).toHaveTextContent(m.password_notice_text());
  },
};

/** Set: how to sign in from now on. */
export const Set: Story = {
  play: async ({ canvasElement, args }) => {
    await setPassword(canvasElement);
    const done = await storyElement(canvasElement, '[data-testid="password-notice-done"]');
    await expect(done).toHaveTextContent(m.password_notice_done({ username: 'example-reader' }));
    await expect(args.onSet).toHaveBeenCalledWith('a long password');
  },
};

/** The session is too old to vouch: through the provider again first. */
export const SignInAgain: Story = {
  args: {
    onSet: fn(() =>
      Promise.reject(new BackendError(403, { reason: 'sign_in_again', detail: 'again' })),
    ),
  },
  play: async ({ canvasElement, args }) => {
    await setPassword(canvasElement);
    await clickStory(canvasElement, '[data-testid="password-notice-provider"]');
    await expect(args.onSignInAgain).toHaveBeenCalledOnce();
  },
};
