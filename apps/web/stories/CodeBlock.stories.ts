import type { Meta, StoryObj } from '@storybook/svelte';
import { expect } from 'storybook/test';

import { CODE_LINES_ATTRIBUTE } from '$lib/editor/code-lines.ts';

import BodySample from './BodySample.svelte';
import { storyElement } from './interactions.ts';

const block = (language: string | null, text: string): object => ({
  type: 'codeBlock',
  ...(language === null ? {} : { attrs: { language } }),
  content: [{ type: 'text', text }],
});

/** Code in three languages, one the highlighter does not know, and one with none. */
const CODE = {
  type: 'doc',
  content: [
    block(
      'ts',
      [
        '// The answer, asked for.',
        'export function ask(question: string): number {',
        '  const answer = 42;',
        '  return question.length > 0 ? answer : -1;',
        '}',
      ].join('\n'),
    ),
    block('python', 'def double(values):\n    return [value * 2 for value in values]  # twice\n'),
    block(
      'rust',
      `fn main() {\n    let words = "${'a long line that wraps '.repeat(6)}";\n    println!("{words}");\n}`,
    ),
    block('klingon', 'qaplah!'),
    block(null, 'plain text, no language'),
  ],
};

/**
 * Code blocks where a body is only read (src/lib/editor/code-view.ts): the
 * language's name and a copy button over each, the colours once the
 * highlighter's worker has them.
 */
const meta = {
  title: 'Editor/Blocks/Code',
  component: BodySample,
  args: { doc: CODE },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '.doc-code .doc-code-keyword');
    await expect(canvasElement.querySelectorAll('.doc-code')).toHaveLength(5);
    await expect(canvasElement.querySelectorAll('button.doc-code-copy')).toHaveLength(5);
  },
} satisfies Meta<typeof BodySample>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Coloured: Story = {};

/** With the reader's line numbers on: out of the text, beside the first row of a line that wraps. */
export const LineNumbers: Story = {
  beforeEach: () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    return () => {
      document.documentElement.removeAttribute(CODE_LINES_ATTRIBUTE);
    };
  },
  play: async ({ canvasElement }) => {
    await storyElement(canvasElement, '.doc-code .doc-code-keyword');
    // One for each of the five lines of the first block.
    await expect(
      canvasElement.querySelectorAll('.doc-code:first-child .doc-code-line'),
    ).toHaveLength(5);
  },
};
