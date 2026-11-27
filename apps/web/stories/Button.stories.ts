import type { Meta, StoryObj } from '@storybook/svelte';

import ButtonSamples from './ButtonSamples.svelte';

const meta = {
  title: 'UI/Button',
  component: ButtonSamples,
} satisfies Meta<typeof ButtonSamples>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Variants: Story = {};
export const Disabled: Story = { args: { disabled: true } };
