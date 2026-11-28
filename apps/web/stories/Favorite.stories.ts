import type { Meta, StoryObj } from '@storybook/svelte';

import FavoriteSample from './FavoriteSample.svelte';

const meta = {
  title: 'Shell/Favorite',
  component: FavoriteSample,
} satisfies Meta<typeof FavoriteSample>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Stars: Story = {};
/** Before the stars are read, and through a public link: no ★ at all. */
export const NotRead: Story = { args: { unread: true } };
