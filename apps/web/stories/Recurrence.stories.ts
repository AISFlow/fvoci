/**
 * How a card repeats: chosen under its due date (none, daily, weekly,
 * monthly, yearly, or every 1 to 365 units from the due date or from the
 * day it is done), shown on its face, and told when completing it made the
 * next card.
 */
import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, within } from 'storybook/test';

import RecurrenceSample from './RecurrenceSample.svelte';

const meta = {
  title: 'Cards/Recurrence',
  component: RecurrenceSample,
} satisfies Meta<typeof RecurrenceSample>;

export default meta;
type Story = StoryObj<typeof meta>;

/** A card that does not repeat: choosing "Custom" opens the count, the unit and the anchor. */
export const None: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.selectOptions(canvas.getByRole('combobox', { name: /repeat|반복/i }), 'custom');
    await expect(canvas.getByTestId('stored-recurrence')).toHaveTextContent('"every":2');
    const every = canvas.getByRole('spinbutton');
    await userEvent.clear(every);
    await userEvent.type(every, '400');
    await userEvent.tab();
    await expect(canvas.getByTestId('stored-recurrence')).toHaveTextContent('"every":365');
  },
};

/** Weekly from the due date: the face of the card carries the mark of it. */
export const Weekly: Story = {
  args: { recurrence: { every: 1, unit: 'week', anchor: 'due' } },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelector('[data-slot="card-repeats"]')).not.toBeNull();
  },
};

/** Every three days, from the day it is done. */
export const Custom: Story = {
  args: { recurrence: { every: 3, unit: 'day', anchor: 'done' } },
};

/** A board with no rule of done or no due date it names: the choices are off, its settings below. */
export const BoardWithoutRules: Story = {
  args: { allowed: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('combobox', { name: /repeat|반복/i })).toBeDisabled();
    await expect(
      canvasElement.querySelector('[data-slot="recurrence-needs-rules"]'),
    ).not.toBeNull();
    await expect(canvas.getByTestId('board-due-date-property')).toBeEnabled();
  },
};

/** Completing it made the next card: the notice, with its undo. */
export const NextCardNotice: Story = {
  args: { notice: true, recurrence: { every: 1, unit: 'week', anchor: 'due' } },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByTestId('next-card-toast')).toHaveTextContent('#12');
  },
};
