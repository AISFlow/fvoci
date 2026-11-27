import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';

import { AUDIT_ACTIONS } from '$lib/logic/audit.ts';

import AuditSample from './AuditSample.svelte';

const meta = {
  title: 'Activity/Audit list',
  component: AuditSample,
  play: async ({ canvasElement, args }) => {
    await expect(canvasElement.querySelectorAll('li')).toHaveLength(
      typeof args.count === 'number' ? args.count : AUDIT_ACTIONS.length,
    );
  },
} satisfies Meta<typeof AuditSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const EveryAction: Story = {};
export const Empty: Story = { args: { count: 0 } };
export const ThreeHundredRecords: Story = { args: { count: 300 } };
export const DeletedPeopleAndHiddenTargets: Story = { args: { hidden: true } };
