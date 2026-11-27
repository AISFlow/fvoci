import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, waitFor, within } from 'storybook/test';
import { m } from '$lib/i18n/paraglide/messages.js';
import PeopleSample from './PeopleSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Workspace/People',
  component: PeopleSample,
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="people-members"]');
  },
} satisfies Meta<typeof PeopleSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Members: Story = {
  play: async ({ canvasElement }) => {
    const list = await storyElement(canvasElement, '[data-testid="people-members"]');
    // A title and credentials are written with the name, which opens the profile.
    await expect(
      within(list).getByRole('button', {
        name: `${m.person_titled({ title: m.person_title_prof(), name: 'Example member 1 · 이름이 긴 예시 구성원' })}, PhD @example-member-1`,
      }),
    ).toHaveAttribute('aria-haspopup', 'dialog');
  },
};
export const Empty: Story = { args: { count: 0 } };
export const ThreeHundredMembers: Story = {
  args: { count: 300 },
  play: async ({ canvasElement }) => {
    const list = await storyElement(canvasElement, '[data-testid="people-members"]');
    await expect(list.children).toHaveLength(300);
  },
};
export const Loading: Story = {
  args: { mode: 'loading' },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="people-example"][data-fixture-ready]');
    await expect(within(canvasElement).getByText(m.app_loading(), { exact: true })).toBeVisible();
    await expect(canvasElement.querySelector('[data-testid="people-members"]')).toBeNull();
  },
};
export const LoadFailed: Story = {
  args: { mode: 'load-error' },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[role="alert"]');
  },
};
export const NotAllowed: Story = { ...LoadFailed, args: { mode: 'forbidden' } };
export const LinkCreated: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="people-make-link"]');
    await expect(await storyElement(canvasElement, '[data-testid="people-link"]')).toHaveValue(
      'https://example.test/invite/example-invitation-3',
    );
  },
};
export const WriteFailed: Story = {
  args: { mode: 'write-error' },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="people-make-link"]');
    await storyElement(canvasElement, '[data-testid="people-problem"]');
  },
};
export const ConfirmRemoval: Story = {
  play: async ({ canvasElement }) => {
    // The row's last button takes the member out; the first opens their profile.
    await clickStory(canvasElement, '[data-member="example-member-1"] > button:last-child');
    await storyElement(canvasElement, '[data-testid="confirm-dialog"]');
  },
};
export const RevokeLink: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="people-links"] button');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="people-links"]')).toBeNull(),
    );
  },
};
