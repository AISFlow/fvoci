import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';
import EmbedSample from './EmbedSample.svelte';
import { embedsSettled, storyElement } from './interactions.ts';

const meta = {
  title: 'Editor/Blocks/Formula',
  component: EmbedSample,
  args: { kind: 'math', source: 'E = mc^2' },
  play: async ({ canvasElement, args }) => {
    if (args.draw === false) {
      await expect(canvasElement.querySelector('[data-testid="embed-example"]')?.textContent).toBe(
        typeof args.source === 'string' ? args.source : '',
      );
    } else {
      const count = typeof args.count === 'number' ? args.count : 1;
      await embedsSettled(canvasElement, count);
      await expect(canvasElement.querySelectorAll('[data-state="drawn"]')).toHaveLength(count);
    }
  },
} satisfies Meta<typeof EmbedSample>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Drawn: Story = {};
/** The actual source slots before the asynchronous renderer paints them. */
export const SourceBeforeDrawing: Story = { args: { draw: false } };
export const Many: Story = { args: { count: 100 } };
export const CannotDraw: Story = {
  args: { source: '\\unknownExampleCommand{x}' },
  play: async ({ canvasElement }) => {
    await expect(
      await storyElement(canvasElement, '[data-state="error"] .doc-embed-problem'),
    ).toBeVisible();
  },
};

/** The browser check holds the renderer request, observes the source, then lets it finish.
 * It owns both assertions so a play deadline cannot race the deliberately held request. */
export const LoadingThenDrawn: Story = { play: () => undefined };
