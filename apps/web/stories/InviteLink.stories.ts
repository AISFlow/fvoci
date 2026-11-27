import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor } from 'storybook/test';
import { m } from '$lib/i18n/paraglide/messages.js';
import InviteLinkSample from './InviteLinkSample.svelte';
import { clickStory } from './interactions.ts';

const meta = { title: 'Workspace/Invitation link', component: InviteLinkSample } satisfies Meta<
  typeof InviteLinkSample
>;
export default meta;
type Story = StoryObj<typeof meta>;
let copiedText = '';
let deniedText = '';
let deniedCalls = 0;

export const ReadyToCopy: Story = {};
export const LongLink: Story = { args: { long: true } };
export const Copied: Story = {
  beforeEach: () => {
    copiedText = '';
    const previous = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: (text: string) => {
          copiedText = text;
          return Promise.resolve();
        },
      },
    });
    return () => {
      if (previous) Object.defineProperty(navigator, 'clipboard', previous);
      else Reflect.deleteProperty(navigator, 'clipboard');
    };
  },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="example-invite-link-copy"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[aria-live]')).toHaveTextContent(
        m['ShareBoard.copiedLink'](),
      ),
    );
    await expect(copiedText).toBe('https://example.test/invite/example-invitation');
  },
};
export const ClipboardDenied: Story = {
  beforeEach: () => {
    deniedText = '';
    deniedCalls = 0;
    const previous = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: (text: string) => {
          deniedCalls += 1;
          deniedText = text;
          return Promise.reject(new Error('Example clipboard refusal'));
        },
      },
    });
    return () => {
      if (previous) Object.defineProperty(navigator, 'clipboard', previous);
      else Reflect.deleteProperty(navigator, 'clipboard');
    };
  },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="example-invite-link-copy"]');
    await waitFor(() => expect(deniedCalls).toBe(1));
    await expect(deniedText).toBe('https://example.test/invite/example-invitation');
    await expect(canvasElement.querySelector('input')).toHaveValue(
      'https://example.test/invite/example-invitation',
    );
    await expect(canvasElement.querySelector('[aria-live]')).toHaveTextContent(
      m.people_invite_note(),
    );
  },
};
