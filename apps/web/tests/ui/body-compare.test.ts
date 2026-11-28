/**
 * The read-only view of a body against the editor: the same document drawn
 * both ways has to come out as the same tree of elements.
 *
 * For every document a real editor is made in the simulated page and its
 * DOM (`editor.view.dom`) is read; `renderBody` draws the same document; both
 * are reduced to a plain shape (element names in order, text, attributes,
 * classes, and whether a checkbox is ticked) and the shapes must be equal.
 * The documents are the 58 that the built-in templates hold and the ones in
 * tests/support/body-docs.ts for everything the templates do not use.
 *
 * A formula and a diagram are compared as the places both sides make for
 * them: the element that holds a formula's source, the code block a diagram
 * is stored as. Drawing into those places is done by one module for both
 * sides (embeds.ts) and is tested on its own; here nothing is drawn yet.
 *
 * WHAT THE COMPARISON LEAVES OUT. This is the whole list; anything else that
 * differs fails the test.
 *
 * On the editor's side, things that exist for editing:
 *
 *  1. The root element itself. Its children are compared. The editor's root
 *     carries `contenteditable`, `role="textbox"`, `translate`, `tabindex` and
 *     the classes `tiptap ProseMirror`; the view's root belongs to the caller.
 *  2. The class `ProseMirror-trailingBreak` on a `<br>`. The `<br>` itself is
 *     compared: the view draws one in the same places (an empty paragraph,
 *     heading or code block; after a line break or a line feed at the end).
 *  3. `<img class="ProseMirror-separator">`: an image without size that the
 *     editor puts before a mention or a line break at the start of a
 *     paragraph or right behind another one, and behind a mention at the end,
 *     so that the cursor has somewhere to be.
 *  4. The `<br>` behind a mention or a formula that ends a paragraph, for the
 *     same reason. It ends a line that already has something on it, so it
 *     adds no line.
 *  5. The class `ProseMirror-selectednode`: the editor's selection, which
 *     starts on a leading image or rule.
 *  6. The placeholder's mark on the empty paragraph or heading the cursor is
 *     in: the attribute `data-placeholder` and the classes `is-empty` and
 *     `is-editor-empty`.
 *  7. `draggable` on an image, a file block and a board view (each can be
 *     dragged to another place) and on a rule while the selection is on it.
 *  8. The `<span>` inside the `<label>` of a task: hidden text that repeats
 *     the checkbox's `aria-label`.
 * 11. A diagram around its source: the editor draws a code block in the
 *     language of diagrams inside an element of its own (`doc-diagram-block`),
 *     with the picture, its button and the place where the source is changed;
 *     only the source in it (its class and name aside) is compared. The view
 *     adds its picture only once the body is in the page (`drawEmbeds`).
 * 12. A formula on a line of its own inside an element of its own in the
 *     editor (`doc-math-block`), which also holds the place where its source
 *     is changed and how to change it; only the formula in it is compared,
 *     without the description that points to that.
 *
 * 13. What a reference shows of what it names: the editor draws it into the
 *     reference as it makes it (its state, and a title or the words for what
 *     the reader may not see); the view makes the place alone, and the same
 *     module draws into it once the body is in the page (reference-chip.ts,
 *     tested on its own). The place is compared: the element, its class and
 *     the identifier.
 * 14. What a file block shows of its file: the editor draws its icon, name,
 *     size and link into the block as it makes it; the view makes the place
 *     alone, and the same module draws into it once the body is in the page
 *     (file-view.ts, tested in editor-file-blocks.test.ts). The place is
 *     compared: the element and the identifier.
 * 15. The bar of a code block: the editor draws the block inside an element
 *     of its own (`doc-code`), with the button that names and changes its
 *     language and the button that copies it above the text; only the block
 *     in it is compared. The view puts its blocks in the same frame, with the
 *     language's name and the copy button, once the body is in the page
 *     (code-view.ts, tested in body-code.test.ts).
 * 19. What a board view shows: the editor draws, inside the frame
 *     (`div.doc-board-view`), the place that the view is drawn into
 *     (`doc-board-view-place`), and says there that the view cannot be seen
 *     where nothing draws it; the view makes the frame alone, and draws into
 *     it once the body is in the page (`mountBoardViews`, tested in
 *     editor-board-views.test.ts). The frame is compared: the element and the
 *     identifier of the view.
 * 17. The frame of a toggle: the editor draws `div.doc-details` around its
 *     chevron's button (`aria-expanded`) and its two parts, which are divs,
 *     and marks it `data-open` while it is open; the view draws the page's
 *     own `<details>`, closed, of a `<summary>` and the div of what it
 *     hides. The parts are compared, the summary as a `<summary>`.
 *
 * On both sides:
 *
 *  9. The wording of the `aria-label` of a task's checkbox. That it is there
 *     is compared. The editor's is its library's English sentence around all
 *     the text in the task; the view's comes from the caller (`taskLabel`).
 *
 * On the view's side, what makes it read-only:
 *
 * 10. `aria-readonly` on a task's checkbox.
 * 18. The icon of a callout: in the editor a button that shows the four
 *     tones to choose from (`doc-callout-controls`), in the view a picture
 *     that says its tone's name (`doc-callout-icon`). The callout and its
 *     blocks are compared.
 *
 * On the view's side, for what is drawn later:
 *
 * 16. `data-line-digits` on a code block (`<pre>`): how wide the room for
 *     the numbers of its lines is, from the first drawing, before the frame
 *     of 15 comes. The editor puts it on that frame.
 *
 * In a browser the editor also sets `contenteditable="false"` on images,
 * rules, mentions and the labels of tasks. The simulated page does not turn
 * that property into an attribute, so it never shows up here.
 */
import { Editor } from '@tiptap/core';
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

import { bodyExtensions } from '$lib/editor/extensions.ts';
import { setFileSource, type FileSource } from '$lib/editor/files.ts';
import { renderBody } from '$lib/editor/render.ts';
import {
  MARK_SPECS,
  NODE_SPECS,
  toStoredDoc,
  validateDoc,
  type DocNode,
} from '$lib/editor/schema.ts';

import { SAMPLES } from './fixtures/body-samples.ts';
import { templateDocuments } from '../support/templates.ts';
import { HAND_MADE, PEOPLE, UNRESOLVED_FILE_PREFIX, WITH_DIAGRAMS } from '../support/body-docs.ts';

// The editor draws a diagram under its source as soon as it has one. The
// diagram library needs a real browser to lay text out; here it is replaced by
// something that draws nothing.
vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: () => Promise.resolve({ ok: false, reason: 'failed' }),
}));

type Shape =
  | string
  | {
      tag: string;
      attributes: Record<string, string>;
      classes: string[];
      checked?: boolean;
      children: Shape[];
    };

/**
 * The numbered list at the top, as data: what is left out, and how often each
 * entry applied in this run. An entry that never applies has no business on
 * the list; a test below fails when there is one. (Entry 1, the root, is not
 * here: only the children of the root are ever looked at.)
 */
const LEFT_OUT = {
  'class ProseMirror-trailingBreak': 0, // 2
  'separator image': 0, // 3
  'break behind a closing mention or formula': 0, // 4
  'class ProseMirror-selectednode': 0, // 5
  'attribute data-placeholder': 0, // 6
  'class is-empty': 0, // 6
  'class is-editor-empty': 0, // 6
  'attribute draggable': 0, // 7
  'hidden name of a checkbox': 0, // 8
  'wording of aria-label': 0, // 9
  'attribute aria-readonly': 0, // 10
  'diagram around its source': 0, // 11
  'block formula around its drawing': 0, // 12
  'what a reference shows': 0, // 13
  'what a file block shows': 0, // 14
  'bar of a code block': 0, // 15
  'attribute data-line-digits': 0, // 16
  'frame of a toggle': 0, // 17
  'icon of a callout': 0, // 18
  'what a board view shows': 0, // 19
};
type LeftOut = keyof typeof LEFT_OUT;
const leaveOut = (entry: LeftOut): void => {
  LEFT_OUT[entry] += 1;
};
const EDITOR_ONLY_CLASSES: Record<string, LeftOut> = {
  'ProseMirror-trailingBreak': 'class ProseMirror-trailingBreak',
  'ProseMirror-selectednode': 'class ProseMirror-selectednode',
  'is-empty': 'class is-empty',
  'is-editor-empty': 'class is-editor-empty',
};
const TASK_NAME = '(the name of the task)';

const isElement = (node: Node | null | undefined): node is Element => node?.nodeType === 1;
const isSeparator = (node: Node | null | undefined): boolean =>
  isElement(node) && node.tagName === 'IMG' && node.classList.contains('ProseMirror-separator');
const isTaskCheckbox = (element: Element): boolean =>
  element.tagName === 'INPUT' &&
  element.parentElement?.parentElement?.hasAttribute('data-checked') === true;

/** Which entry of the list takes this node of the editor out of the comparison, if any. */
function leftOutNode(node: Element): LeftOut | undefined {
  if (isSeparator(node)) return 'separator image';
  if (node.tagName === 'BR' && node.classList.contains('ProseMirror-trailingBreak')) {
    let before = node.previousSibling;
    while (isSeparator(before)) before = before?.previousSibling ?? null;
    if (
      isElement(before) &&
      (before.hasAttribute('data-mention') ||
        before.hasAttribute('data-reference') ||
        before.classList.contains('doc-math'))
    )
      return 'break behind a closing mention or formula';
  }

  if (node.tagName === 'SPAN' && node.parentElement?.tagName === 'LABEL')
    return 'hidden name of a checkbox';
  return undefined;
}

/** Which entry of the list takes this attribute out of the comparison, if any. */
function leftOutAttribute(node: Element, name: string, ofEditor: boolean): LeftOut | undefined {
  if (ofEditor && name === 'data-placeholder') return 'attribute data-placeholder';
  const draggable =
    node.tagName === 'IMG' ||
    node.tagName === 'HR' ||
    node.hasAttribute('data-file-block') ||
    node.hasAttribute('data-board-view');
  if (ofEditor && name === 'draggable' && draggable) return 'attribute draggable';
  if (!ofEditor && name === 'aria-readonly' && isTaskCheckbox(node))
    return 'attribute aria-readonly';
  if (!ofEditor && name === 'data-line-digits' && node.tagName === 'PRE')
    return 'attribute data-line-digits';
  return undefined;
}

/** What is compared of a node. `side` says whose DOM it is; see the list at the top. */
function shapeOf(node: Node, side: 'editor' | 'view'): Shape | undefined {
  if (node.nodeType === 3) return node.nodeValue ?? '';
  if (!isElement(node)) throw new Error(`unexpected node of type ${node.nodeType}`);
  const ofEditor = side === 'editor';

  if (ofEditor && node.tagName === 'DIV' && node.classList.contains('doc-diagram-block')) {
    leaveOut('diagram around its source');
    const source = node.querySelector(':scope > pre.doc-diagram-source');
    if (!source) throw new Error('a diagram without its source');
    source.classList.remove('doc-diagram-source');
    source.querySelector('code')?.removeAttribute('aria-label');
    return shapeOf(source, side);
  }
  if (ofEditor && node.tagName === 'DIV' && node.hasAttribute('data-details')) {
    leaveOut('frame of a toggle');
    const parts = node.querySelector(':scope > .doc-details-parts');
    const summary = parts?.querySelector(':scope > [data-details-summary]');
    const content = parts?.querySelector(':scope > [data-details-content]');
    const drawn = summary ? shapeOf(summary, side) : undefined;
    const inside = content ? shapeOf(content, side) : undefined;
    if (drawn === undefined || typeof drawn === 'string' || inside === undefined)
      throw new Error('a toggle without its parts');
    return {
      tag: 'details',
      attributes: { 'data-details': '' },
      classes: ['doc-details'],
      children: [{ ...drawn, tag: 'summary' }, inside],
    };
  }
  const iconOfCallout = ofEditor ? 'doc-callout-controls' : 'doc-callout-icon';
  if (node.parentElement?.hasAttribute('data-callout') && node.classList.contains(iconOfCallout)) {
    leaveOut('icon of a callout');
    return undefined;
  }
  if (ofEditor && node.tagName === 'DIV' && node.classList.contains('doc-code')) {
    leaveOut('bar of a code block');
    const block = node.querySelector(':scope > pre');
    if (!block) throw new Error('a code block without its text');
    return shapeOf(block, side);
  }
  if (ofEditor && node.tagName === 'DIV' && node.classList.contains('doc-math-block')) {
    leaveOut('block formula around its drawing');
    const formula = node.querySelector(':scope > .doc-math');
    if (!formula) throw new Error('a block formula without its drawing');
    // Its description (how to edit it) points into the holder.
    formula.removeAttribute('aria-describedby');
    return shapeOf(formula, side);
  }
  if (ofEditor && node.hasAttribute('data-file-block') && node.hasAttribute('data-drawn')) {
    leaveOut('what a file block shows');
    const place = node.cloneNode(false) as Element;
    for (const name of ['data-drawn', 'data-state', 'role', 'aria-label']) {
      place.removeAttribute(name);
    }
    return shapeOf(place, side);
  }
  if (ofEditor && node.hasAttribute('data-board-view') && node.childElementCount > 0) {
    leaveOut('what a board view shows');
    return shapeOf(node.cloneNode(false), side);
  }
  if (ofEditor && node.hasAttribute('data-reference') && node.hasAttribute('data-drawn')) {
    leaveOut('what a reference shows');
    const place = node.cloneNode(false) as Element;
    for (const name of ['data-drawn', 'data-state', 'data-kind', 'aria-busy']) {
      place.removeAttribute(name);
    }
    return shapeOf(place, side);
  }
  const without = ofEditor ? leftOutNode(node) : undefined;
  if (without) {
    leaveOut(without);
    return undefined;
  }

  const attributes: Record<string, string> = {};
  for (const { name, value } of Array.from(node.attributes)) {
    if (name === 'class') continue;
    const dropped = leftOutAttribute(node, name, ofEditor);
    if (dropped) {
      leaveOut(dropped);
    } else if (name === 'aria-label' && isTaskCheckbox(node)) {
      if (ofEditor) leaveOut('wording of aria-label');
      attributes[name] = TASK_NAME;
    } else {
      attributes[name] = value;
    }
  }
  const classes: string[] = [];
  for (const name of Array.from(node.classList)) {
    const dropped = ofEditor ? EDITOR_ONLY_CLASSES[name] : undefined;
    if (dropped) leaveOut(dropped);
    else classes.push(name);
  }
  classes.sort();
  const tag = node.tagName.toLowerCase();

  const shape: Shape = {
    tag,
    attributes,
    classes,
    children: Array.from(node.childNodes)
      .map((child) => shapeOf(child, side))
      .filter((child) => child !== undefined),
  };
  if (node instanceof HTMLInputElement) shape.checked = node.checked;
  return shape;
}

const shapesOf = (parent: ParentNode, side: 'editor' | 'view'): Shape[] =>
  Array.from(parent.childNodes)
    .map((child) => shapeOf(child, side))
    .filter((child) => child !== undefined);

let editor: Editor | undefined;
let previousResolver: FileSource;

beforeAll(() => {
  previousResolver = setFileSource({
    url: (fileId) =>
      fileId.startsWith(UNRESOLVED_FILE_PREFIX) ? undefined : `/files/board-1/${fileId}`,
  });
});
afterAll(() => {
  setFileSource(previousResolver);
});
afterEach(() => {
  editor?.destroy();
  editor = undefined;
});

function drawWithEditor(doc: DocNode): Editor {
  editor = new Editor({
    element: document.createElement('div'),
    extensions: bodyExtensions({
      mention: { search: () => [], labelOf: (userId) => PEOPLE[userId] ?? userId },
    }),
    content: doc,
  });
  return editor;
}

const drawWithView = (doc: DocNode) =>
  renderBody(doc, {
    resolveUser: (userId) => PEOPLE[userId],
    taskLabel: (text) => `Task: ${text || 'empty'}`,
  });

/** The documents that were compared and came out equal, and how often each kind of node and mark occurs in them. */
const compared = {
  documents: 0,
  nodes: new Map<string, number>(),
  marks: new Map<string, number>(),
};

function count(node: DocNode): void {
  compared.nodes.set(node.type, (compared.nodes.get(node.type) ?? 0) + 1);
  for (const mark of node.marks ?? [])
    compared.marks.set(mark.type, (compared.marks.get(mark.type) ?? 0) + 1);
  for (const child of node.content ?? []) count(child);
}

function expectSameShape(doc: DocNode): void {
  expect(validateDoc(doc)).toEqual([]);
  const drawn = drawWithEditor(doc);
  const view = drawWithView(doc);
  if (!view.ok) throw new Error(`not drawn: ${view.problems.join(' / ')}`);
  expect(view.empty).toBe(false);
  expect(shapesOf(view.fragment, 'view')).toEqual(shapesOf(drawn.view.dom, 'editor'));
  compared.documents += 1;
  count(doc);
}

const templates = templateDocuments();

describe('the view draws what the editor draws', () => {
  it('has the 110 documents of the built-in templates to compare', () => {
    expect(templates).toHaveLength(110);
  });

  it.each(
    templates.map((entry, index) => [`${index + 1}. ${entry.template} / ${entry.card}`, entry.doc]),
  )('template document %s', (_name, doc) => {
    expectSameShape(doc);
    // The editor took the document as it is, so both drew the same thing. In
    // the stored form: some templates write the start of a list at 1, which
    // the stored form leaves out.
    expect(toStoredDoc(editor?.getJSON() as DocNode)).toEqual(toStoredDoc(doc));
  });

  it.each(Object.entries(HAND_MADE))('hand-made document: %s', (_name, doc) => {
    expectSameShape(doc);
  });

  it.each(Object.entries(WITH_DIAGRAMS))('document with diagrams: %s', (_name, doc) => {
    expectSameShape(doc);
    // The editor took the document as it is: a diagram is an ordinary code block to it.
    expect(toStoredDoc(editor?.getJSON() as DocNode)).toEqual(doc);
  });

  it('the sample of every node and mark', () => {
    expectSameShape(SAMPLES.everything);
  });

  // The three tests below look at what went through the comparisons above,
  // so they mean something only when the whole file runs.

  it('compared every document', () => {
    expect(compared.documents).toBe(
      110 + Object.keys(HAND_MADE).length + Object.keys(WITH_DIAGRAMS).length + 1,
    );
  });

  it('compared every kind of node and mark on the list at least once', () => {
    const times = (counts: Map<string, number>, types: string[]) =>
      types.map((type) => [type, (counts.get(type) ?? 0) > 0]);
    const nodeTypes = Object.keys(NODE_SPECS);
    const markTypes = Object.keys(MARK_SPECS);
    expect(nodeTypes).toHaveLength(28);
    expect(markTypes).toHaveLength(5);
    expect(times(compared.nodes, nodeTypes)).toEqual(nodeTypes.map((type) => [type, true]));
    expect(times(compared.marks, markTypes)).toEqual(markTypes.map((type) => [type, true]));
    expect([...compared.nodes.keys()].sort()).toEqual([...nodeTypes].sort());
    expect([...compared.marks.keys()].sort()).toEqual([...markTypes].sort());
  });

  it('has, among the hand-made documents, each thing the templates do not use', () => {
    const anywhere = (node: DocNode, test: (node: DocNode) => boolean): boolean =>
      test(node) || (node.content ?? []).some((child) => anywhere(child, test));
    const has = (test: (node: DocNode) => boolean): boolean =>
      Object.values(HAND_MADE).some((doc) => anywhere(doc, test));
    const marked = (type: string) => (node: DocNode) =>
      (node.marks ?? []).some((mark) => mark.type === type);
    expect({
      quote: has((node) => node.type === 'blockquote'),
      codeWithLanguage: has((node) => node.type === 'codeBlock' && node.attrs?.language === 'ts'),
      codeWithoutLanguage: has((node) => node.type === 'codeBlock' && node.attrs === undefined),
      lineBreak: has((node) => node.type === 'hardBreak'),
      mention: has((node) => node.type === 'mention'),
      struck: has(marked('strike')),
      inlineCode: has(marked('code')),
      listFromThree: has((node) => node.type === 'orderedList' && node.attrs?.start === 3),
      taskInTask: has(
        (node) =>
          node.type === 'taskItem' &&
          (node.content ?? []).some((child) => child.type === 'taskList'),
      ),
      imageWithDescription: has(
        (node) =>
          node.type === 'image' && typeof node.attrs?.alt === 'string' && node.attrs.alt !== '',
      ),
      severalMarks: has((node) => (node.marks ?? []).length >= 3),
      emptyParagraph: has((node) => node.type === 'paragraph' && node.content === undefined),
      formulaInALine: has((node) => node.type === 'inlineMath'),
      formulaOnItsOwnLine: has((node) => node.type === 'blockMath'),
      formulaEndingAParagraph: has(
        (node) => node.type === 'paragraph' && node.content?.at(-1)?.type === 'inlineMath',
      ),
      cellOverSeveralColumns: has((node) => typeof node.attrs?.colspan === 'number'),
      cellOverSeveralRows: has((node) => typeof node.attrs?.rowspan === 'number'),
      rowWithoutCells: has((node) => node.type === 'tableRow' && node.content === undefined),
    }).toEqual({
      quote: true,
      codeWithLanguage: true,
      codeWithoutLanguage: true,
      lineBreak: true,
      mention: true,
      struck: true,
      inlineCode: true,
      listFromThree: true,
      taskInTask: true,
      imageWithDescription: true,
      severalMarks: true,
      emptyParagraph: true,
      formulaInALine: true,
      formulaOnItsOwnLine: true,
      formulaEndingAParagraph: true,
      cellOverSeveralColumns: true,
      cellOverSeveralRows: true,
      rowWithoutCells: true,
    });
  });

  it('left nothing out of the comparison that it did not have to', () => {
    const unused = Object.entries(LEFT_OUT)
      .filter(([, times]) => times === 0)
      .map(([entry]) => entry);
    expect(unused).toEqual([]);
  });
});

describe('where the editor and the view are set up alike', () => {
  it('nests marks in the order of the editor', () => {
    // render.ts keeps this order by hand; here it is read from the editor.
    const { schema } = drawWithEditor({ type: 'doc', content: [{ type: 'paragraph' }] });
    const marks = ['code', 'strike', 'italic', 'bold', 'link'].map((type) =>
      type === 'link' ? { type, attrs: { href: '/x' } } : { type },
    );
    const view = renderBody({
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'x', marks }] }],
    });
    if (!view.ok) throw new Error('not drawn');
    const nesting: string[] = [];
    for (
      let at = view.fragment.querySelector('p')?.firstElementChild;
      at;
      at = at.firstElementChild
    )
      nesting.push(at.tagName.toLowerCase());
    const tags: Record<string, string> = {
      link: 'a',
      bold: 'strong',
      italic: 'em',
      strike: 's',
      code: 'code',
    };
    expect(nesting).toEqual(Object.keys(schema.marks).map((type) => tags[type]));
  });

  it('gives a link the attributes the editor gives it', () => {
    const doc: DocNode = {
      type: 'doc',
      content: [
        {
          type: 'paragraph',
          content: [
            { type: 'text', text: 'x', marks: [{ type: 'link', attrs: { href: '/board/1' } }] },
          ],
        },
      ],
    };
    const fromEditor = drawWithEditor(doc).view.dom.querySelector('a');
    const view = renderBody(doc);
    if (!view.ok) throw new Error('not drawn');
    const fromView = view.fragment.querySelector('a');
    const attributes = (element: Element | null | undefined) =>
      Object.fromEntries(
        Array.from(element?.attributes ?? []).map(({ name, value }) => [name, value]),
      );
    expect(attributes(fromView)).toEqual(attributes(fromEditor));
    expect(attributes(fromView)).toEqual({
      href: '/board/1',
      target: '_blank',
      rel: 'noopener noreferrer nofollow',
    });
  });

  it('shows the identifier of a user it has no name for, as the editor does', () => {
    const doc: DocNode = {
      type: 'doc',
      content: [
        {
          type: 'paragraph',
          content: [{ type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c003' } }],
        },
      ],
    };
    const fromEditor = drawWithEditor(doc).view.dom.querySelector('[data-mention]');
    const view = drawWithView(doc);
    if (!view.ok) throw new Error('not drawn');
    expect(view.fragment.querySelector('[data-mention]')?.textContent).toBe(
      '@01930000-0000-7000-8000-00000000c003',
    );
    expect(fromEditor?.textContent).toBe('@01930000-0000-7000-8000-00000000c003');
  });
});
