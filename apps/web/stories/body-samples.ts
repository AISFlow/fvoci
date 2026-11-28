/** Made-up bodies for the stories of the read-only view. */
import type { BodyReferences } from '$lib/editor/body-editor.ts';
import type { FileDetails, FileSource } from '$lib/editor/files.ts';
import type { ShownReference } from '$lib/editor/reference-chip.ts';
import type { DocNode } from '$lib/editor/schema.ts';

const text = (value: string, ...marks: string[]): DocNode =>
  marks.length > 0
    ? { type: 'text', text: value, marks: marks.map((type) => ({ type })) }
    : { type: 'text', text: value };
const paragraph = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const heading = (level: number, value: string): DocNode => ({
  type: 'heading',
  attrs: { level },
  content: [text(value)],
});
const item = (...content: DocNode[]): DocNode => ({ type: 'listItem', content });
const task = (checked: boolean, ...content: DocNode[]): DocNode => ({
  type: 'taskItem',
  attrs: { checked },
  content,
});

export const PEOPLE: Record<string, string> = {
  '01930000-0000-7000-8000-00000000c001': '예시 구성원',
  '01930000-0000-7000-8000-00000000c002': 'Example guest',
};

/** A picture for a file identifier that starts with `file-`; the others have no address. */
function sampleFileUrl(fileId: string): string | undefined {
  if (!fileId.startsWith('file-')) return undefined;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="160">` +
    `<rect width="480" height="160" fill="hsl(210 60% 80%)"/>` +
    `<text x="240" y="88" text-anchor="middle" font-size="24" font-family="sans-serif">${fileId}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

/** What the stories know of the files of file blocks; one that starts with `gone-` is not there. */
const SAMPLE_FILES: Record<string, FileDetails> = {
  'file-minutes.pdf': {
    name: '2026년 11월 운영위원회 회의록과 첨부 자료 모음 (최종본).pdf',
    size: 482_304,
  },
  'file-budget.xlsx': { name: 'budget.xlsx', size: 18_944 },
  'file-photos.zip': { name: 'photos.zip', size: 52_428_800 },
};

/** The stories' files: pictures made up from the identifier, and the records above. */
export const sampleFileSource: FileSource = {
  url: sampleFileUrl,
  details: (fileId) =>
    Promise.resolve(
      fileId.startsWith('gone-') ? null : (SAMPLE_FILES[fileId] ?? { name: fileId, size: 1024 }),
    ),
};

/** Every kind of node and mark a body may contain. */
export const MIXED: DocNode = {
  type: 'doc',
  content: [
    heading(1, '출시 준비 회의'),
    paragraph(
      text('다음 주 '),
      text('목요일', 'bold'),
      text('까지 끝낼 일입니다. 담당은 '),
      { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c001' } },
      text(' 님, 검토는 '),
      { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c002' } },
      text(' 님, 그리고 이름을 모르는 '),
      { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c003' } },
      text('.'),
    ),
    heading(2, '할 일'),
    {
      type: 'taskList',
      content: [
        task(true, paragraph(text('번역 검수')), {
          type: 'taskList',
          content: [task(true, paragraph(text('한국어'))), task(false, paragraph(text('日本語')))],
        }),
        task(false, paragraph(text('설치 문서 고치기'))),
        task(false, paragraph(text('Release notes'))),
      ],
    },
    heading(3, 'Notes'),
    {
      type: 'bulletList',
      content: [
        item(
          paragraph(
            text('A list entry with '),
            text('italic', 'italic'),
            text(', '),
            text('struck', 'strike'),
            text(' and '),
            text('bold italic', 'bold', 'italic'),
            text(' text.'),
          ),
        ),
        item(paragraph(text('두 번째 항목')), {
          type: 'orderedList',
          attrs: { start: 3 },
          content: [item(paragraph(text('셋부터 세는 목록'))), item(paragraph(text('넷')))],
        }),
      ],
    },
    { type: 'blockquote', content: [paragraph(text('인용한 문장은 이렇게 보입니다.'))] },
    {
      type: 'codeBlock',
      attrs: { language: 'ts' },
      content: [text('const answer: number = 42; // 주석\nconsole.warn(answer);')],
    },
    paragraph(
      text('Inline '),
      text('code', 'code'),
      text(' and a '),
      { type: 'text', text: 'link', marks: [{ type: 'link', attrs: { href: '/board/example' } }] },
      text('.'),
      { type: 'hardBreak' },
      text('A second line in the same paragraph.'),
    ),
    { type: 'paragraph' },
    paragraph(text('위는 빈 문단입니다.')),
    { type: 'horizontalRule' },
    { type: 'image', attrs: { fileId: 'file-chart', alt: '진행 그래프' } },
    { type: 'image', attrs: { fileId: 'no-address', alt: '주소를 풀 수 없는 파일' } },
  ],
};

/**
 * One document from the built-in templates: the template "Project schedule",
 * the card "Agree on the scope".
 */
export const FROM_TEMPLATE: DocNode = {
  type: 'doc',
  content: [
    heading(2, 'Goals'),
    paragraph(text('Write down what must be delivered and what can wait for the next phase.')),
    { type: 'horizontalRule' },
    heading(2, 'Checklist'),
    {
      type: 'taskList',
      content: [
        task(true, paragraph(text('Confirm deliverables and deadline'))),
        task(false, paragraph(text('Assign owners for each phase'))),
        task(false, paragraph(text('Share the schedule with everyone involved'))),
      ],
    },
  ],
};

/** A body the validator refuses: a node that is not on the list, and a link that runs a script. */
export const INVALID: unknown = {
  type: 'doc',
  content: [
    paragraph(text('이 문단은 멀쩡하지만 문서 전체가 그려지지 않습니다.')),
    { type: 'table', content: [paragraph(text('cell'))] },
    paragraph({
      type: 'text',
      text: 'click',
      marks: [{ type: 'link', attrs: { href: 'javascript:alert(1)' } }],
    }),
  ],
};

const math = (latex: string): DocNode => ({ type: 'inlineMath', attrs: { latex } });
const mathBlock = (latex: string): DocNode => ({ type: 'blockMath', attrs: { latex } });
const diagram = (source: string): DocNode => ({
  type: 'codeBlock',
  attrs: { language: 'mermaid' },
  content: [text(source)],
});

/** Formulas, in a line and on lines of their own; the last two cannot be drawn. */
export const FORMULAS: DocNode = {
  type: 'doc',
  content: [
    heading(2, '수식'),
    paragraph(
      text('문장 안의 수식 '),
      math('E = mc^2'),
      text(' 과 한글이 든 수식 '),
      math('속도 = \\frac{거리}{시간}'),
      text(', 그리고 한 줄을 차지하는 수식:'),
    ),
    mathBlock('x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}'),
    mathBlock(
      '\\begin{aligned} \\nabla \\cdot \\mathbf{E} &= \\frac{\\rho}{\\varepsilon_0} \\\\ \\nabla \\cdot \\mathbf{B} &= 0 \\end{aligned}',
    ),
    {
      type: 'bulletList',
      content: [
        item(paragraph(text('목록 안에서 '), math('\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}'))),
      ],
    },
    paragraph(text('그릴 수 없는 수식은 원문과 안내가 보입니다: '), math('\\frac{1}{')),
    mathBlock('\\href{https://example.test}{링크는 만들어지지 않습니다}'),
  ],
};

/** Diagrams: code blocks in the language of diagrams. The last cannot be read. */
export const DIAGRAMS: DocNode = {
  type: 'doc',
  content: [
    heading(2, '다이어그램'),
    diagram('flowchart LR\n  A[초안] --> B{검토}\n  B -->|통과| C[배포]\n  B -->|수정| A'),
    diagram(
      'sequenceDiagram\n  participant 웹\n  participant 서버\n  웹->>서버: 저장\n  Note over 서버: 검증\n  서버-->>웹: 200',
    ),
    diagram('pie title 진행\n  "끝남" : 12\n  "하는 중" : 5\n  "남음" : 8'),
    diagram(
      'gantt\n  title 일정\n  dateFormat YYYY-MM-DD\n  section 준비\n  설계 :a1, 2026-11-01, 10d\n  구현 :after a1, 20d',
    ),
    paragraph(text('읽을 수 없는 다이어그램은 원문과 안내가 보입니다:')),
    diagram('flowchart LR\n  A --> --> B ]]'),
  ],
};

const toggle = (summary: string, ...content: DocNode[]): DocNode => ({
  type: 'details',
  content: [
    summary === ''
      ? { type: 'detailsSummary' }
      : { type: 'detailsSummary', content: [text(summary)] },
    { type: 'detailsContent', content },
  ],
});
const callout = (tone: string, ...content: DocNode[]): DocNode => ({
  type: 'callout',
  attrs: { tone },
  content,
});
const cell = (type: string, value: string): DocNode => ({
  type,
  content: [paragraph(text(value))],
});

/**
 * Toggles and callouts: a question whose answer is folded away, one that
 * hides a table, a callout and another toggle, one with no summary, and a
 * callout of each tone.
 */
export const TOGGLES: DocNode = {
  type: 'doc',
  content: [
    heading(2, '토글'),
    toggle(
      '분산은 왜 음수가 아닌가?',
      paragraph(text('제곱의 평균이라서, 어느 제곱도 음수가 아니다.')),
    ),
    toggle(
      'What this toggle hides',
      paragraph(text('A table, a callout and another toggle:')),
      {
        type: 'table',
        content: [
          {
            type: 'tableRow',
            content: [cell('tableHeader', 'Kind'), cell('tableHeader', 'Where')],
          },
          {
            type: 'tableRow',
            content: [cell('tableCell', 'Toggle'), cell('tableCell', 'Anywhere')],
          },
        ],
      },
      callout('tip', paragraph(text('A callout inside a toggle.'))),
      toggle('Deeper', paragraph(text('A toggle inside a toggle.'))),
    ),
    toggle('', paragraph(text('This toggle has no summary.'))),
    heading(2, '콜아웃'),
    callout('info', paragraph(text('참고: ', 'bold'), text('A note, set apart.'))),
    callout('tip', paragraph(text('팁: 아이콘을 누르면 종류가 바뀝니다.'))),
    callout('warn', paragraph(text('Warning: ')), {
      type: 'bulletList',
      content: [
        item(paragraph(text('a list in a callout'))),
        item(paragraph(text('and another line'))),
      ],
    }),
    callout('danger', paragraph(text('위험: 되돌릴 수 없습니다.'))),
  ],
};

const PLAN = '01930000-0000-7000-8000-00000000b001';
const LONG = '01930000-0000-7000-8000-00000000b002';
const PAGE = '01930000-0000-7000-8000-00000000b003';
const UNSEEN = '01930000-0000-7000-8000-00000000b004';
const ASKED = '01930000-0000-7000-8000-00000000b005';

/** What the stories know of what references name: `null` for what the reader may not see. */
const NAMED: Record<string, ShownReference | null> = {
  [PLAN]: { title: '분기 계획', icon: '🚀', kind: 'card', href: '#plan' },
  [LONG]: {
    title: 'A title that goes on for much longer than a narrow screen is wide, and then some',
    icon: '',
    kind: 'card',
    href: '#long',
  },
  [PAGE]: { title: 'Handbook', icon: '📘', kind: 'page' },
  [UNSEEN]: null,
};

/**
 * The stories have no server: what a reference names is made up here. One
 * identifier is never answered, as while the server is being asked.
 */
export const SAMPLE_REFERENCES: BodyReferences = {
  shown: (id) => (id === ASKED ? undefined : (NAMED[id] ?? null)),
  search: (query) =>
    Object.entries(NAMED)
      .flatMap(([id, shown]) => (shown ? [{ value: id, label: shown.title }] : []))
      .filter((entry) => entry.label.toLowerCase().includes(query.toLowerCase())),
  named: () => undefined,
  seen: (id) => Promise.resolve(Boolean(NAMED[id])),
};

const reference = (id: string): DocNode => ({ type: 'reference', attrs: { id } });
const words = (text: string): DocNode => ({ type: 'text', text });

/**
 * References in every state: a card with its icon, a title too long for the
 * line, a page (which has no screen to open), what the reader may not see,
 * and one still being asked.
 */
export const REFERENCES: DocNode = {
  type: 'doc',
  content: [
    {
      type: 'paragraph',
      content: [words('See '), reference(PLAN), words(' and '), reference(PAGE), words('.')],
    },
    { type: 'paragraph', content: [reference(LONG)] },
    {
      type: 'paragraph',
      content: [
        words('Not for this reader: '),
        reference(UNSEEN),
        words(' Asked: '),
        reference(ASKED),
      ],
    },
    {
      type: 'heading',
      attrs: { level: 2 },
      content: [reference(PLAN), words(' in a heading')],
    },
  ],
};

const fileBlock = (fileId: string): DocNode => ({ type: 'file', attrs: { fileId } });

/** File blocks: a document, a sheet, an archive, a name too long for the line, and a file that is gone. */
export const FILES: DocNode = {
  type: 'doc',
  content: [
    paragraph(text('회의 자료')),
    fileBlock('file-minutes.pdf'),
    fileBlock('file-budget.xlsx'),
    fileBlock('file-photos.zip'),
    fileBlock('gone-draft.pdf'),
  ],
};
