import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, within } from 'storybook/test';

import { m } from '$lib/i18n/paraglide/messages.js';
import { humanFileSize } from '$lib/logic/format.ts';

import TransfersSample from './TransfersSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

/** The stack of uploads and archives at the bottom right of the page (features/shell/Transfers). */
const meta = {
  title: 'Shell/Transfers',
  component: TransfersSample,
} satisfies Meta<typeof TransfersSample>;
export default meta;
type Story = StoryObj<typeof meta>;

const NAME = '회의록 2026-11 · Minutes of the long example meeting.hwp';
const MIB = 1024 * 1024;

/** A narrow window has two or more as one card: its list of each is opened first. */
async function eachShown(canvas: HTMLElement): Promise<void> {
  const group = canvas.ownerDocument.querySelector<HTMLElement>('[data-transfer-group]');
  if (group)
    await userEvent.click(within(group).getByRole('button', { name: m.shell_uploads_each() }));
}

/** One on its way: how far, in a bar and in bytes; it is announced as it starts, not as it goes. */
export const Progress: Story = {
  play: async ({ canvasElement }) => {
    const card = await storyElement(canvasElement, '[data-transfer="upload"]');
    await expect(card).toHaveTextContent(m.shell_transfer_upload({ name: NAME }));
    await expect(card).toHaveTextContent(
      m.shell_transfer_progress({ done: humanFileSize(8 * MIB), total: humanFileSize(17 * MIB) }),
    );
    await expect(within(card).getByRole('progressbar')).toHaveAttribute('value', String(8 * MIB));
    const said = await storyElement(canvasElement, '[data-testid="transfers-said"]');
    await expect(said).toHaveTextContent(m.shell_transfer_upload({ name: NAME }));
  },
};

/** Closing one on its way asks first. */
export const AskingToStop: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-transfer="upload"] [data-close]');
    const card = await storyElement(canvasElement, '[data-transfer="upload"]');
    await expect(card).toHaveTextContent(m.shell_transfer_ask());
    await expect(
      within(card).getByRole('button', { name: m.shell_transfer_ask_keep() }),
    ).toHaveFocus();
  },
};

/** In: it stays a moment, and while the focus is in the stack. */
export const Done: Story = {
  args: { shown: 'done' },
  play: async ({ canvasElement }) => {
    const card = await storyElement(canvasElement, '[data-transfer="upload"][data-finished]');
    await expect(card).toHaveTextContent(m.shell_transfer_done_upload({ name: 'scan.pdf' }));
    within(card)
      .getByRole('button', { name: m.shell_transfer_dismiss({ name: 'scan.pdf' }) })
      .focus();
  },
};

/** Failed: each stays with why, and one that sending again may get past can be sent again. */
export const Failed: Story = {
  args: { shown: 'failed' },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-transfers]');
    await eachShown(canvasElement);
    const large = await storyElement(canvasElement, '[data-failed="too-large"]');
    await expect(large).toHaveTextContent(
      m.shell_upload_failure_too_large({ limit: humanFileSize(100 * MIB) }),
    );
    // Sending the same file again would be refused again: it is not offered.
    await expect(
      within(large).queryByRole('button', {
        name: m.shell_transfer_retry_name({ name: 'video.mp4' }),
      }),
    ).toBeNull();
    const network = await storyElement(canvasElement, '[data-failed="network"]');
    await expect(network).toHaveTextContent(m.shell_upload_failure_network());
    await expect(
      within(network).getByRole('button', {
        name: m.shell_transfer_retry_name({ name: 'notes.txt' }),
      }),
    ).toBeVisible();
  },
};

/** Many at once: one card, with each behind a button. */
export const Many: Story = {
  args: { shown: 'many' },
  play: async ({ canvasElement }) => {
    const group = await storyElement(canvasElement, '[data-transfer-group]');
    await expect(group).toHaveTextContent(m.shell_uploads_running({ count: 6 }));
    await userEvent.click(within(group).getByRole('button', { name: m.shell_uploads_each() }));
    await expect(group.querySelectorAll('[data-transfer="upload"]')).toHaveLength(6);
  },
};
