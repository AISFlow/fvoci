import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';
import ViewSample from './ViewSample.svelte';
import { clickStory, storyElement, visibleStoryElement } from './interactions.ts';

const meta = {
  title: 'Views/Calendar',
  component: ViewSample,
  args: { kind: 'calendar' },
  play: async ({ canvasElement, args }) => {
    const calendar = await storyElement(canvasElement, '[data-testid="calendar"][data-layout]');
    await expect(calendar).toHaveAttribute('data-month', '2026-11');
    if (args.readOnly) {
      await expect(
        calendar.querySelectorAll('[data-grip], [data-new-card], [data-testid="day-new-card"]'),
      ).toHaveLength(0);
    } else if (calendar.dataset.layout === 'bars' && args.count !== 0) {
      await expect(calendar.querySelectorAll('[data-grip]').length).toBeGreaterThan(0);
    }
  },
} satisfies Meta<typeof ViewSample>;
export default meta;
type Story = StoryObj<typeof meta>;
export const MonthGrid: Story = {};
export const Empty: Story = { args: { count: 0 } };
export const MiniMonth: Story = { args: { narrow: true } };
export const ThreeHundredCards: Story = { args: { count: 300 } };
export const ReadOnly: Story = { args: { readOnly: true } };
export const WeekGrid: Story = { args: { week: true } };
export const WeekAgenda: Story = { args: { week: true, narrow: true } };
export const DayList: Story = {
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="calendar"][data-layout]');
    await clickStory(
      canvasElement,
      '[data-day="2026-11-13"][data-day-button], [data-day="2026-11-13"] [data-day-button]',
    );
    await expect(
      await visibleStoryElement(canvasElement, '[data-testid="day-list"]'),
    ).toBeVisible();
  },
};
export const NoDate: Story = {
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="calendar"][data-layout]');
    await clickStory(canvasElement, '[data-testid="calendar-undated"]');
    await expect(
      await visibleStoryElement(canvasElement, '[data-testid="undated-list"]'),
    ).toBeVisible();
  },
};
