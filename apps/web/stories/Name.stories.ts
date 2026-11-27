import type { Meta, StoryObj } from '@storybook/svelte';
import { expect, userEvent, waitFor } from 'storybook/test';
import NameSample from './NameSample.svelte';
import { clickStory, storyElement } from './interactions.ts';

const meta = {
  title: 'Workspace/Name dialog',
  component: NameSample,
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '[data-testid="name-dialog"]');
  },
} satisfies Meta<typeof NameSample>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Empty: Story = {};
export const ExistingName: Story = { args: { initial: 'Example workspace · 예시 작업 공간' } };
export const LongName: Story = {
  args: { initial: 'Example workspace · 긴 이름 '.repeat(4).slice(0, 100) },
};
export const WhitespaceCannotSubmit: Story = {
  args: { initial: '   ' },
  play: async ({ canvasElement }) => {
    await expect(
      await storyElement(canvasElement, '[data-testid="name-dialog"] [type="submit"]'),
    ).toBeDisabled();
  },
};
export const Submitted: Story = {
  play: async ({ canvasElement }) => {
    await userEvent.type(
      await storyElement(canvasElement, '[data-testid="name-dialog"] input'),
      'New example workspace',
    );
    await clickStory(canvasElement, '[data-testid="name-dialog"] [type="submit"]');
    await waitFor(() =>
      expect(canvasElement.querySelector('[data-testid="submitted-name"]')).toHaveTextContent(
        'New example workspace',
      ),
    );
  },
};
