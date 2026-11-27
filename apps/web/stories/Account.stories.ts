import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';
import AccountSample from './AccountSample.svelte';

const meta = { title: 'Account/Fields', component: AccountSample } satisfies Meta<
  typeof AccountSample
>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Empty: Story = {};
export const Filled: Story = { args: { populated: true } };
export const LongName: Story = { args: { populated: true, long: true } };
export const FieldErrors: Story = {
  args: { populated: true, invalid: true },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelectorAll('[role="alert"]')).toHaveLength(4);
    await expect(canvasElement.querySelectorAll('[aria-invalid="true"]')).toHaveLength(4);
  },
};
export const Submitting: Story = { args: { populated: true, busy: true } };
