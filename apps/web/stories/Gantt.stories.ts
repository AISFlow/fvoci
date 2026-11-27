import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';
import ViewSample from './ViewSample.svelte';
import { clickStory, storyElement, visibleStoryElement } from './interactions.ts';

const meta = {
  title: 'Views/Gantt',
  component: ViewSample,
  args: { kind: 'gantt' },
  play: async ({ canvasElement, args }) => {
    await expect(await storyElement(canvasElement, '[data-testid="gantt-axis"]')).toBeVisible();
    const grips = canvasElement.querySelectorAll('[data-grip]');
    if (args.readOnly) await expect(grips).toHaveLength(0);
    else if (args.count !== 0) await expect(grips.length).toBeGreaterThan(0);
  },
} satisfies Meta<typeof ViewSample>;
export default meta;
type Story = StoryObj<typeof meta>;
export const AxisBarsAndArrows: Story = {
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-arrow]');
    await expect(canvasElement.querySelectorAll('[data-bar]')).toHaveLength(7);
  },
};
export const Empty: Story = { args: { count: 0 } };
export const ThreeHundredCards: Story = { args: { count: 300 } };
export const ReadOnly: Story = { args: { readOnly: true } };
export const Narrow: Story = { args: { narrow: true } };
export const LatePredecessor: Story = {
  args: { late: true },
  play: async ({ canvasElement }) => {
    await expect(await storyElement(canvasElement, '[data-mark="late"]')).toBeVisible();
  },
};
export const Cycle: Story = {
  args: { cycle: true },
  play: async ({ canvasElement }) => {
    await expect(await storyElement(canvasElement, '[data-mark="loop"]')).toBeVisible();
  },
};
export const NoDate: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-testid="gantt-undated"]');
    await expect(
      await visibleStoryElement(canvasElement, '[data-testid="undated-list"]'),
    ).toBeVisible();
  },
};
export const FourKindsAndLag: Story = {
  args: { variedRelations: true },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-arrow][data-kind="ss"]');
    await expect(
      canvasElement.querySelectorAll('[data-arrow][data-kind="fs"]').length,
    ).toBeGreaterThan(0);
    await expect(
      canvasElement.querySelectorAll('[data-arrow][data-kind="ss"]').length,
    ).toBeGreaterThan(0);
    await expect(
      canvasElement.querySelectorAll('[data-arrow][data-kind="ff"]').length,
    ).toBeGreaterThan(0);
    await expect(
      canvasElement.querySelectorAll('[data-arrow][data-kind="sf"]').length,
    ).toBeGreaterThan(0);
    await expect(canvasElement.querySelectorAll('[data-arrow-trigger]').length).toBeGreaterThan(0);
  },
};
