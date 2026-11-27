import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, waitFor } from 'storybook/test';
import { m } from '$lib/i18n/paraglide/messages.js';
import WorkspaceSample from './WorkspaceSample.svelte';
import { clickStory, storyElement, visibleStoryElement } from './interactions.ts';

const meta = { title: 'Workspace/Switcher', component: WorkspaceSample } satisfies Meta<
  typeof WorkspaceSample
>;
export default meta;
type Story = StoryObj<typeof meta>;

export const CurrentWorkspace: Story = {};
export const Empty: Story = { args: { count: 0 } };
export const OneWorkspace: Story = { args: { count: 1 } };
export const LongName: Story = { args: { long: true } };
export const TwoHundredWorkspaces: Story = {
  args: { count: 200, long: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    await storyElement(canvasElement, '[data-action="example-workspace-199"]');
  },
};
export const SwitchWorkspace: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    await clickStory(canvasElement, '[data-action="example-workspace-2"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="workspace-choice"]')).toHaveTextContent(
        'example-workspace-2',
      ),
    );
  },
};

/** The own space first, with a mark of its own, whatever the list's order. */
export const WithOwnSpace: Story = {
  args: { count: 2, own: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    const first = await storyElement(canvasElement, '[data-action]');
    await expect(first).toHaveAttribute('data-action', 'own-space');
    await expect(first.querySelector('svg')).not.toBeNull();
  },
};

/** In the own space: the head names it, marked, and its line is the current one. */
export const InOwnSpace: Story = {
  args: { count: 2, own: true, inOwn: true },
  play: async ({ canvasElement }) => {
    const head = await storyElement(canvasElement, '[data-testid="workspace-switcher"]');
    await expect(head).toHaveAttribute('data-own-space');
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    const own = await storyElement(canvasElement, '[data-action="own-space"]');
    await expect(own.getAttribute('aria-label')).toBeTruthy();
  },
};

/** The own space of an account with a password, to a provider's session: it says how it opens. */
export const OwnSpaceLocked: Story = {
  args: { count: 2, own: true, locked: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    const own = await storyElement(canvasElement, '[data-action="own-space"]');
    await expect(own).toHaveAttribute('aria-label', m.own_space_locked_label());
    await expect(own).toHaveTextContent(m.own_space_name());
  },
};

/** In the own space, locked: its line says both how it opens and that it is the one shown. */
export const InOwnSpaceLocked: Story = {
  args: { count: 2, own: true, inOwn: true, locked: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    const own = await storyElement(canvasElement, '[data-action="own-space"]');
    await expect(own).toHaveAttribute(
      'aria-label',
      m.shell_workspace_current({ name: m.own_space_locked_label() }),
    );
  },
};

/** An account in no organization: the switcher has its own space and the way to start one. */
export const OwnSpaceAlone: Story = {
  args: { count: 0, own: true, inOwn: true, starts: 'made' },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    await clickStory(canvasElement, '[data-action="new-organization"]');
    const name = await visibleStoryElement(canvasElement, '[data-testid="new-organization-name"]');
    await userEvent.type(name, 'Design');
    await clickStory(canvasElement, '[data-testid="new-organization-submit"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="workspace-choice"]')).toHaveTextContent(
        'started-0',
      ),
    );
    await expect(
      canvasElement.querySelector('[data-testid="workspace-switcher"]'),
    ).toHaveTextContent('Design');
  },
};

/** Past the organizations an account may start: the dialog says so with the server's number. */
export const OrganizationLimit: Story = {
  args: { count: 3, own: true, starts: 'limit' },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="workspace-switcher"]');
    await clickStory(canvasElement, '[data-action="new-organization"]');
    const name = await visibleStoryElement(canvasElement, '[data-testid="new-organization-name"]');
    await userEvent.type(name, 'Fourth');
    await clickStory(canvasElement, '[data-testid="new-organization-submit"]');
    const problem = await visibleStoryElement(
      canvasElement,
      '[data-testid="new-organization-problem"]',
    );
    await expect(problem).toHaveTextContent('3');
    // Refused, the name is in use again and has the focus, to be changed or left.
    await waitFor(() => expect(name).toHaveFocus());
    await expect(
      canvasElement.ownerDocument.querySelector('[data-testid="new-organization-dialog"]'),
    ).not.toBeNull();
  },
};
