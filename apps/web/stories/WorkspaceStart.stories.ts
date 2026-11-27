import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, fn } from 'storybook/test';
import WorkspaceStart from '$lib/features/shell/WorkspaceStart.svelte';
import { m } from '$lib/i18n/paraglide/messages.js';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Workspace/Empty workspace',
  component: WorkspaceStart,
  args: { own: false, onAddBoard: fn() },
} satisfies Meta<typeof WorkspaceStart>;
export default meta;
type Story = StoryObj<typeof meta>;

/** The own space with nothing in it: what it is, and the way to make a board. */
export const OwnSpace: Story = {
  args: { own: true },
  play: async ({ canvasElement, args }) => {
    const start = await storyElement(canvasElement, '[data-testid="workspace-start"]');
    await expect(start.querySelector('h1')).toHaveTextContent(m.own_space_name());
    await expect(start).toHaveTextContent(m.own_space_start());
    await clickStory(canvasElement, '[data-testid="workspace-start-add"]');
    await expect(args.onAddBoard).toHaveBeenCalledOnce();
  },
};

/** A shared workspace with nothing in it yet. */
export const SharedWorkspace: Story = {
  play: async ({ canvasElement }) => {
    const start = await storyElement(canvasElement, '[data-testid="workspace-start"]');
    await expect(start.querySelector('h1')).toBeNull();
    await expect(start).toHaveTextContent(m.app_no_boards());
  },
};

/** A guest makes nothing: no way to the picker. */
export const NothingToMake: Story = {
  args: { onAddBoard: undefined },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="workspace-start"]');
    await expect(canvasElement.querySelector('[data-testid="workspace-start-add"]')).toBeNull();
  },
};
