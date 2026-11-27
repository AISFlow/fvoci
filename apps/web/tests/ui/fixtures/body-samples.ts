/**
 * A document with every node and mark a body knows, formulas among them: one
 * the editor and the read-only view are compared on (body-compare.test.ts),
 * beside the templates and the hand-made corners of tests/support/body-docs.ts.
 */
import type { DocNode } from '$lib/editor/schema.ts';

const text = (value: string, ...marks: string[]): DocNode =>
  marks.length > 0
    ? { type: 'text', text: value, marks: marks.map((type) => ({ type })) }
    : { type: 'text', text: value };
const paragraph = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const item = (...content: DocNode[]): DocNode => ({ type: 'listItem', content });
const task = (checked: boolean, label: string): DocNode => ({
  type: 'taskItem',
  attrs: { checked },
  content: [paragraph(text(label))],
});

/** Every node and mark a body had before formulas: no formula and no diagram in it. */
const PLAIN: DocNode[] = [
  { type: 'heading', attrs: { level: 1 }, content: [text('출시 준비 회의')] },
  paragraph(
    text('다음 주 '),
    text('목요일', 'bold'),
    text('까지 끝낼 일을 정리했습니다. 담당은 '),
    { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c001' } },
    text(' 님, 검토는 '),
    { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c002' } },
    text(' 님입니다.'),
  ),
  { type: 'heading', attrs: { level: 2 }, content: [text('할 일')] },
  {
    type: 'taskList',
    content: [
      task(true, '번역 검수'),
      task(false, '설치 문서 고치기'),
      task(false, 'Release notes'),
    ],
  },
  { type: 'heading', attrs: { level: 2 }, content: [text('Notes')] },
  {
    type: 'bulletList',
    content: [
      item(
        paragraph(
          text('A list entry with '),
          text('italic', 'italic'),
          text(' and '),
          text('struck', 'strike'),
          text(' text.'),
        ),
      ),
      item(paragraph(text('두 번째 항목')), {
        type: 'orderedList',
        attrs: { start: 1 },
        content: [item(paragraph(text('안쪽 하나'))), item(paragraph(text('안쪽 둘')))],
      }),
    ],
  },
  { type: 'blockquote', content: [paragraph(text('인용한 문장은 이렇게 보입니다.'))] },
  {
    type: 'codeBlock',
    attrs: { language: 'ts' },
    content: [text('const answer: number = 42; // 주석')],
  },
  paragraph(
    text('Inline '),
    text('code', 'code'),
    text(' and a '),
    {
      type: 'text',
      text: 'link',
      marks: [{ type: 'link', attrs: { href: '/board/example' } }],
    },
    text('.'),
    { type: 'hardBreak' },
    text('A second line in the same paragraph.'),
  ),
  { type: 'horizontalRule' },
  { type: 'image', attrs: { fileId: 'file-chart', alt: '진행 그래프' } },
];

export const SAMPLES: Record<'everything', DocNode> = {
  everything: {
    type: 'doc',
    content: [
      ...PLAIN,
      paragraph(
        text('피타고라스의 정리 '),
        { type: 'inlineMath', attrs: { latex: 'a^2 + b^2 = c^2' } },
        text(' 를 씁니다.'),
      ),
      { type: 'blockMath', attrs: { latex: '\\int_0^1 x^2 \\, dx = \\frac{1}{3}' } },
    ],
  },
};
