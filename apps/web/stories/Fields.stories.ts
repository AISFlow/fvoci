import type { Meta, StoryObj } from '@storybook/svelte';

import FieldSamples from './FieldSamples.svelte';

const meta = {
  title: 'UI/Fields',
  component: FieldSamples,
} satisfies Meta<typeof FieldSamples>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Field outlines and the focus indicator on the page and on a muted surface. */
export const Outlines: Story = {};
