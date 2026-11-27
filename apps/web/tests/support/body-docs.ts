/**
 * Documents written by hand for comparing the read-only view of a body with
 * the editor. The built-in templates use only part of what a body may
 * contain; these hold the rest, and the corners of what they do use: quotes,
 * code, line breaks, mentions, struck text, inline code, a list that starts
 * at 3, tasks inside tasks, images with a description, text under several
 * marks, empty paragraphs, and formulas. Diagrams are in a list of their own
 * (`WITH_DIAGRAMS`, at the end).
 */
import type { DocMark, DocNode } from '$lib/editor/schema.ts';

type MarkLike = string | DocMark;

export const link = (href: string): DocMark => ({ type: 'link', attrs: { href } });

export const text = (value: string, ...marks: MarkLike[]): DocNode =>
  marks.length > 0
    ? {
        type: 'text',
        text: value,
        marks: marks.map((mark) => (typeof mark === 'string' ? { type: mark } : mark)),
      }
    : { type: 'text', text: value };

export const paragraph = (...content: DocNode[]): DocNode =>
  content.length > 0 ? { type: 'paragraph', content } : { type: 'paragraph' };
export const heading = (level: number, ...content: DocNode[]): DocNode =>
  content.length > 0
    ? { type: 'heading', attrs: { level }, content }
    : { type: 'heading', attrs: { level } };
export const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });
export const item = (...content: DocNode[]): DocNode => ({ type: 'listItem', content });
export const bullets = (...content: DocNode[]): DocNode => ({ type: 'bulletList', content });
export const numbered = (start: number | undefined, ...content: DocNode[]): DocNode =>
  start === undefined
    ? { type: 'orderedList', content }
    : { type: 'orderedList', attrs: { start }, content };
export const tasks = (...content: DocNode[]): DocNode => ({ type: 'taskList', content });
export const task = (checked: boolean, ...content: DocNode[]): DocNode => ({
  type: 'taskItem',
  attrs: { checked },
  content,
});
export const quote = (...content: DocNode[]): DocNode => ({ type: 'blockquote', content });
export const code = (language: string | undefined, value?: string): DocNode => ({
  type: 'codeBlock',
  ...(language === undefined ? {} : { attrs: { language } }),
  ...(value === undefined ? {} : { content: [text(value)] }),
});
export const rule = (): DocNode => ({ type: 'horizontalRule' });
export const lineBreak = (): DocNode => ({ type: 'hardBreak' });
export const image = (fileId: string, alt?: string): DocNode => ({
  type: 'image',
  attrs: alt === undefined ? { fileId } : { fileId, alt },
});
/** A file block: a file to download, named by its identifier. */
export const fileBlock = (fileId: string): DocNode => ({ type: 'file', attrs: { fileId } });
export const mention = (userId: string): DocNode => ({ type: 'mention', attrs: { userId } });
/** A reference to a card or a page. */
export const reference = (id: string): DocNode => ({ type: 'reference', attrs: { id } });
/** A formula inside a line. */
export const math = (latex: string): DocNode => ({ type: 'inlineMath', attrs: { latex } });
/** A formula on a line of its own. */
export const mathBlock = (latex: string): DocNode => ({ type: 'blockMath', attrs: { latex } });
export const table = (...content: DocNode[]): DocNode => ({ type: 'table', content });
export const row = (...content: DocNode[]): DocNode =>
  content.length > 0 ? { type: 'tableRow', content } : { type: 'tableRow' };
const cellOf =
  (type: string) =>
  (spans: { colspan?: number; rowspan?: number }, ...content: DocNode[]): DocNode =>
    Object.keys(spans).length > 0 ? { type, attrs: spans, content } : { type, content };
export const cell = cellOf('tableCell');
export const header = cellOf('tableHeader');

/** The people the documents mention. The third (gone) is deliberately not among them. */
export const PEOPLE: Record<string, string> = {
  '01930000-0000-7000-8000-00000000c001': '김하늘',
  '01930000-0000-7000-8000-00000000c002': 'Ada Lovelace',
};

/** Files whose identifier starts like this have no address. */
export const UNRESOLVED_FILE_PREFIX = 'missing-';

export const HAND_MADE: Record<string, DocNode> = {
  'headings of every level': doc(
    heading(1, text('제목 하나')),
    heading(2, text('Heading two')),
    heading(3, text('Heading three with '), text('bold', 'bold')),
    paragraph(text('Text under the headings.')),
  ),

  quotes: doc(
    quote(paragraph(text('인용한 문장.'))),
    quote(
      paragraph(text('A quote with a list:')),
      bullets(item(paragraph(text('one'))), item(paragraph(text('two')))),
      quote(paragraph(text('and a quote inside the quote'))),
    ),
  ),

  'code blocks': doc(
    code('ts', 'const answer: number = 42; // 주석'),
    code(undefined, 'no language\n  two spaces in\n\ttab in'),
    code('c++', 'int main() { return 0; }'),
    code('rust'),
    code(undefined),
    code('sh', 'ends with a newline\n'),
    code(undefined, '<script>alert("not markup")</script> &amp; <b>'),
  ),

  'line breaks': doc(
    paragraph(text('first line'), lineBreak(), text('second line')),
    paragraph(text('a break at the end'), lineBreak()),
    paragraph(lineBreak(), text('a break at the start')),
    paragraph(text('two breaks'), lineBreak(), lineBreak(), text('in a row')),
    paragraph(lineBreak()),
    heading(2, text('In a heading'), lineBreak(), text('too')),
  ),

  mentions: doc(
    paragraph(
      text('담당 '),
      mention('01930000-0000-7000-8000-00000000c001'),
      text(' 님, 검토 '),
      mention('01930000-0000-7000-8000-00000000c002'),
      text(', 그리고 떠난 사람 '),
      mention('01930000-0000-7000-8000-00000000c003'),
    ),
    paragraph(mention('01930000-0000-7000-8000-00000000c001')),
    paragraph(
      mention('01930000-0000-7000-8000-00000000c001'),
      mention('01930000-0000-7000-8000-00000000c002'),
    ),
    heading(3, mention('01930000-0000-7000-8000-00000000c002'), text(' in a heading')),
    paragraph(
      text('bold', 'bold'),
      mention('01930000-0000-7000-8000-00000000c001'),
      text('bold again', 'bold'),
    ),
  ),

  references: doc(
    paragraph(
      text('see '),
      reference('01930000-0000-7000-8000-00000000b001'),
      text(' and '),
      reference('01930000-0000-7000-8000-00000000b002'),
    ),
    paragraph(reference('01930000-0000-7000-8000-00000000b001')),
    heading(2, reference('01930000-0000-7000-8000-00000000b002'), text(' in a heading')),
    paragraph(
      text('bold', 'bold'),
      reference('01930000-0000-7000-8000-00000000b001'),
      text('bold again', 'bold'),
    ),
  ),

  'struck text and inline code': doc(
    paragraph(
      text('This is '),
      text('struck', 'strike'),
      text(' and this is '),
      text('code()', 'code'),
      text('.'),
    ),
    paragraph(text('<b>not bold</b> & "quoted"', 'code')),
  ),

  'numbered lists': doc(
    numbered(3, item(paragraph(text('three'))), item(paragraph(text('four')))),
    numbered(1, item(paragraph(text('one')))),
    numbered(undefined, item(paragraph(text('no start given')))),
    numbered(0, item(paragraph(text('zero')))),
    numbered(
      41,
      item(
        paragraph(text('forty-one')),
        numbered(7, item(paragraph(text('seven, inside'))), item(paragraph(text('eight')))),
      ),
    ),
  ),

  'bullet lists in each other': doc(
    bullets(
      item(
        paragraph(text('one')),
        bullets(
          item(
            paragraph(text('two')),
            bullets(
              item(
                paragraph(text('three')),
                bullets(item(paragraph(text('four')), paragraph(text('a second paragraph')))),
              ),
            ),
          ),
        ),
      ),
      item(paragraph(text('back at the top')), code('txt', 'code in a list'), rule()),
    ),
  ),

  'tasks inside tasks': doc(
    tasks(
      task(
        true,
        paragraph(text('done, with tasks inside')),
        tasks(
          task(false, paragraph(text('inner, open'))),
          task(
            true,
            paragraph(text('inner, done')),
            tasks(task(false, paragraph(text('innermost')))),
          ),
        ),
      ),
      task(false, paragraph(text('open, with a second paragraph')), paragraph(text('more'))),
      task(false, paragraph()),
      task(
        true,
        paragraph(
          text('for '),
          mention('01930000-0000-7000-8000-00000000c001'),
          lineBreak(),
          text('with '),
          text('marks', 'bold'),
        ),
        bullets(item(paragraph(text('a plain list in a task')))),
        image('file-in-task', 'in a task'),
      ),
    ),
    bullets(
      item(paragraph(text('a plain item')), tasks(task(true, paragraph(text('a task in it'))))),
    ),
  ),

  images: doc(
    image('file-first', '진행 그래프'),
    paragraph(text('between')),
    image('file-no-alt'),
    image('file-empty-alt', ''),
    image(`${UNRESOLVED_FILE_PREFIX}file`, 'no address for this one'),
    image(`${UNRESOLVED_FILE_PREFIX}other`),
    image('file-quotes', 'a "quoted" <description> & more'),
    quote(image('file-in-quote', 'in a quote')),
  ),

  'text under several marks': doc(
    paragraph(
      text('all four', link('https://example.com/a?b=1&c="d"'), 'bold', 'italic', 'strike'),
      text(' bold italic', 'bold', 'italic'),
      text(' italic', 'italic'),
      text(' plain '),
      text('italic', 'italic'),
      text(' italic bold', 'italic', 'bold'),
      text(' bold', 'bold'),
    ),
    // The stored order of marks has no meaning: these are out of the editor's order.
    paragraph(
      text('strike first', 'strike', 'italic', 'bold', link('/board/1')),
      text(' then code', 'code', link('/board/1')),
      text(' same link', link('/board/1')),
      text(' other link', link('/board/2')),
      text(' mail', link('mailto:someone@example.com')),
    ),
    paragraph(
      text('bold ', 'bold'),
      text('bold struck', 'bold', 'strike'),
      text(' bold', 'bold'),
      text(' struck', 'strike'),
      text(' struck italic', 'strike', 'italic'),
    ),
    // Two texts with the same marks, which the editor joins into one.
    paragraph(text('joined ', 'bold'), text('together', 'bold'), text(' and '), text('plain')),
    heading(2, text('Marks in a '), text('heading', 'italic', 'code')),
  ),

  'empty paragraphs': doc(
    paragraph(),
    paragraph(text('after an empty paragraph')),
    paragraph(),
    paragraph(),
    heading(2),
    paragraph(text('before the last, empty one')),
    paragraph(),
  ),

  'only empty paragraphs': doc(paragraph(), paragraph()),

  'spaces and characters that look like markup': doc(
    paragraph(text('two  spaces,   three, and a trailing one ')),
    paragraph(text('  leading spaces')),
    paragraph(text('a\ttab and a line\nfeed inside one text')),
    paragraph(text('ends with a line feed\n')),
    paragraph(text('<script>alert(1)</script> <img src=x onerror=alert(1)> &lt; &amp; "\'`')),
    paragraph(text('한글 조합 문자 한글, emoji 🚀👍🏽, and a zero width​space')),
  ),

  'rules and what follows them': doc(
    rule(),
    rule(),
    paragraph(text('between rules')),
    rule(),
    image('file-after-rule'),
    rule(),
  ),

  'an image first': doc(image('file-lead', 'first'), paragraph(text('after the image'))),

  'file blocks': doc(
    fileBlock('7minutes.pdf'),
    paragraph(text('between')),
    fileBlock('7minutes.pdf'),
    bullets(item(paragraph(text('in a list')), fileBlock('7report.hwp'))),
    quote(fileBlock('7sheet.xlsx')),
    fileBlock(`${UNRESOLVED_FILE_PREFIX}file.pdf`),
  ),

  formulas: doc(
    paragraph(
      text('에너지는 '),
      math('E = mc^2'),
      text(' 이고, 한글이 든 식은 '),
      math('속도 = v'),
      text('.'),
    ),
    paragraph(math('a^2 + b^2 = c^2')),
    paragraph(text('a formula at the end '), math('\\frac{1}{2}')),
    paragraph(math('x'), math('y')),
    mathBlock('\\int_0^1 x^2 \\, dx = \\frac{1}{3}'),
    heading(2, text('In a heading '), math('x_i')),
    bullets(item(paragraph(text('in a list '), math('\\sqrt{2}')), mathBlock('\\sum_{i=1}^{n} i'))),
    quote(mathBlock('e^{i\\pi} + 1 = 0')),
    tasks(task(false, paragraph(math('n!'), text(' in a task')))),
    paragraph(text('bold ', 'bold'), math('x'), text(' bold again', 'bold')),
    // What looks like markup is text to the formula library as well.
    paragraph(math('\\text{<script>alert(1)</script>}')),
    // Not a formula that can be drawn: the source stays, with a notice.
    mathBlock('\\frac{1}{'),
    paragraph(text('and one in a line that cannot be drawn '), math('\\nosuchcommand')),
  ),

  tables: doc(
    paragraph(text('before')),
    table(
      row(header({}, paragraph(text('Name'))), header({ colspan: 2 }, paragraph(text('Role')))),
      row(
        cell({ rowspan: 2 }, paragraph(text('Ada', 'bold'))),
        cell({}, paragraph()),
        cell({}, bullets(item(paragraph(text('in a list'))))),
      ),
      row(cell({ colspan: 2 }, code('ts', 'let x = 1;'), paragraph(text('after code')))),
    ),
    // A row covered whole by the cells above has no cell of its own.
    table(row(cell({ rowspan: 2 }, paragraph(text('tall')))), row()),
    paragraph(text('after')),
  ),
};

/**
 * Documents with diagrams: code blocks in the language of diagrams. They are
 * kept apart from `HAND_MADE` because the editor and the read-only view show
 * a diagram differently on purpose (the editor keeps the source in sight),
 * so they cannot go into a comparison of layout.
 */
export const WITH_DIAGRAMS: Record<string, DocNode> = {
  'a diagram between paragraphs': doc(
    paragraph(text('before the diagram')),
    code('mermaid', 'flowchart LR\n  A[시작] --> B{분기}\n  B -->|예| C[끝]\n  B -->|아니오| A'),
    paragraph(text('after the diagram')),
  ),
  'several diagrams, and code that is none': doc(
    code('mermaid', 'sequenceDiagram\n  Alice->>Bob: 안녕\n  Bob-->>Alice: 응'),
    code('ts', 'const notADiagram = true;'),
    code('mermaid', 'pie title 비율\n  "가" : 40\n  "나" : 35\n  "다" : 25'),
    bullets(
      item(
        paragraph(text('in a list')),
        code('mermaid', 'stateDiagram-v2\n  [*] --> A\n  A --> [*]'),
      ),
    ),
    // Cannot be read as a diagram: the source stays, with a notice.
    code('mermaid', 'flowchart LR\n  A --> --> B ]]'),
    // Nothing to draw yet.
    code('mermaid'),
  ),
};
