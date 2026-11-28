import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';
import ViewSample from './ViewSample.svelte';
import { clickStory, storyElement, visibleStoryElement } from './interactions.ts';

const meta = {
  title: 'Views/Header and tools',
  component: ViewSample,
  args: { kind: 'table', headerOnly: true },
  play: async ({ canvasElement, args }) => {
    await expect(await storyElement(canvasElement, '[data-testid="view-header"]')).toBeVisible();
    const editing = canvasElement.querySelectorAll(
      '[data-tool]:not([data-tool="search"]):not([data-tool="search-close"]):not([data-tool="views"]), [data-testid="new-card"]',
    );
    if (args.readOnly) await expect(editing).toHaveLength(0);
    else await expect(editing.length).toBeGreaterThan(0);
  },
} satisfies Meta<typeof ViewSample>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Tools: Story = {};
export const ReadOnly: Story = { args: { readOnly: true } };
export const Narrow: Story = { args: { narrow: true } };
export const Filter: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-tool="filter"]');
    await expect(
      await visibleStoryElement(canvasElement, '[data-testid="filter-editor"]'),
    ).toBeVisible();
  },
};
async function openAside(canvas: HTMLElement, tool: string): Promise<void> {
  const header = await storyElement(canvas, '[data-testid="view-header"]');
  if (header.dataset.layout === 'row') await clickStory(canvas, `[data-tool="${tool}"]`);
  else {
    await clickStory(canvas, '[data-tool="more"]');
    await clickStory(canvas, `[data-action="${tool}"]`);
  }
}
export const Sort: Story = {
  play: async ({ canvasElement }) => {
    await openAside(canvasElement, 'sort');
    await expect(
      await visibleStoryElement(canvasElement, '[data-testid="sort-editor"]'),
    ).toBeVisible();
  },
};
export const Properties: Story = {
  play: async ({ canvasElement }) => {
    await openAside(canvasElement, 'properties');
    await expect(
      await visibleStoryElement(canvasElement, '[data-testid="properties-panel"]'),
    ).toBeVisible();
  },
};
/** One view: the button names it all the same, and its list offers the kinds of view to add. */
export const Views: Story = {
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-tool="views"]');
    const menu = await visibleStoryElement(canvasElement, '[role="menu"]');
    await expect(menu.querySelectorAll('a[role="menuitem"]')).toHaveLength(1);
    await expect(menu.querySelector('a[role="menuitem"]')).toHaveAttribute('aria-current', 'page');
    await expect(menu.querySelector('[data-action="add-board"]')).not.toBeNull();
    await expect(menu.querySelector('[data-action="rename"]')).not.toBeNull();
    // The last view of a board cannot be deleted: a line of the dropdown or a button of the sheet.
    await expect(
      menu.querySelector('[data-action="delete"]')?.matches('[aria-disabled="true"], :disabled'),
    ).toBe(true);
  },
};
/** A reader: the list has the views and nothing that changes them. */
export const ReadOnlyViews: Story = {
  args: { readOnly: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-tool="views"]');
    const menu = await visibleStoryElement(canvasElement, '[role="menu"]');
    await expect(menu.querySelectorAll('[role="menuitem"]')).toHaveLength(1);
    await expect(menu.querySelectorAll('a[role="menuitem"]')).toHaveLength(1);
  },
};
/** A board that offers every quick filter: none on, then two, then none again. */
export const QuickFilters: Story = {
  args: { quick: true },
  play: async ({ canvasElement }) => {
    const row = await storyElement(canvasElement, '[data-testid="quick-filters"]');
    const chips = [...row.querySelectorAll<HTMLElement>('[data-quick-filter]')];
    await expect(chips.map((chip) => chip.dataset.quickFilter)).toEqual([
      'mine',
      'incomplete',
      'overdue',
      'dueThisWeek',
    ]);
    for (const chip of chips) await expect(chip).toHaveAttribute('aria-pressed', 'false');
    await clickStory(canvasElement, '[data-quick-filter="incomplete"]');
    await clickStory(canvasElement, '[data-quick-filter="overdue"]');
    await expect(row.querySelector('[data-quick-filter="overdue"]')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(
      await storyElement(canvasElement, '[data-quick-clear] [data-count]'),
    ).toHaveTextContent('2');
    await clickStory(canvasElement, '[data-quick-clear]');
    await expect(row.querySelector('[data-quick-clear]')).toBeNull();
  },
};
/** In a pane 390 pixels wide the chips are one line that scrolls sideways. */
export const QuickFiltersNarrow: Story = {
  args: { quick: true, narrow: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-quick-filter="mine"]');
    await clickStory(canvasElement, '[data-quick-filter="incomplete"]');
    const row = await storyElement(canvasElement, '[data-testid="quick-filters"]');
    const tops = new Set(
      [...row.querySelectorAll('button')].map((each) =>
        Math.round(each.getBoundingClientRect().top),
      ),
    );
    await expect(tops.size).toBe(1);
    await expect(row.getBoundingClientRect().width).toBeLessThanOrEqual(390);
    await clickStory(canvasElement, '[data-quick-clear]');
  },
};
/** A filter with a group of conditions, a date within a period and whether a card is done. */
export const FilterGroups: Story = {
  args: { quick: true, filtered: true },
  play: async ({ canvasElement }) => {
    await clickStory(canvasElement, '[data-tool="filter"]');
    const editor = await visibleStoryElement(canvasElement, '[data-testid="filter-editor"]');
    const group = await storyElement(canvasElement, '[data-filter-group]');
    await expect(group.querySelectorAll('[data-filter-row]')).toHaveLength(2);
    await expect(group.querySelector('[data-slot="filter-period"]')).not.toBeNull();
    await expect(editor.querySelectorAll(':scope [data-filter-row]')).toHaveLength(3);
    await expect(editor.querySelector('[data-tool="add-group"]')).not.toBeNull();
  },
};
