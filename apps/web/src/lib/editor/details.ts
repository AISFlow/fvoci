/**
 * The toggle of a body: a line that is always shown (its summary) over
 * blocks it shows only while it is open (its content). Notes of a meeting
 * fold their past items in one, a page of questions its answers, a unit of
 * a course the solution to its exercise.
 *
 * The document holds the two parts and nothing else (`schema.ts`): whether
 * a toggle is open is no attribute of it. It is how each reader's own page
 * shows it (details-open.ts), so that opening the answer to a question
 * opens it for nobody else, and reading a body writes nothing to it.
 *
 * A toggle stands where a table does, in the document itself and in what
 * another toggle hides, and holds what the document holds: blocks, tables,
 * callouts and toggles. It stands in no quote, list entry, cell or callout.
 *
 * The keys, past what every block does:
 *
 * - Enter in the summary goes on in the content, with what was after the
 *   caret on a line of its own at its start; the toggle opens.
 * - Backspace at the start of the summary makes the toggle lines again:
 *   its summary a paragraph, its content after it.
 * - Backspace at the start of what the toggle hides goes back to the end of
 *   its summary, and takes an empty line with it where there are others.
 * - Enter on an empty last line of the content leaves the toggle: the line
 *   is put after it, or a new one where it was the only line.
 *
 * Chrome on Android leaves Enter to the page (ProseMirror takes its keydown
 * there for a part of composing), which would split the summary into a toggle
 * of its own: the `beforeinput` it sends for the new line does what Enter does.
 * Only there: on iOS ProseMirror takes Enter from the change the page makes,
 * and from no change, so a new line refused there would be a second Enter.
 */
import { mergeAttributes, Node, type KeyboardShortcutCommand } from '@tiptap/core';
import { Fragment, type Node as DocumentNode, type ResolvedPos } from '@tiptap/pm/model';
import { Plugin, TextSelection, type EditorState, type Transaction } from '@tiptap/pm/state';

import { detailsOpenPlugin, detailsView, openAt } from './details-open.ts';
import { DOCUMENT_ONLY } from './table.ts';

export const DETAILS = 'details';
export const DETAILS_SUMMARY = 'detailsSummary';
export const DETAILS_CONTENT = 'detailsContent';

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    details: {
      /** Makes the line of the caret a toggle, the line its summary, and opens it. */
      setDetails: () => ReturnType;
      /** Makes the toggle around the caret lines again: its summary, then its content. */
      unsetDetails: () => ReturnType;
      /** One or the other: a toggle where there is none, lines where there is one. */
      toggleDetails: () => ReturnType;
    };
  }
}

/** Whether blocks that stand only in the document may stand in this node. */
export const takesDocumentBlocks = (node: DocumentNode): boolean =>
  node.type.name === 'doc' || node.type.name === DETAILS_CONTENT;

/** The depth of the innermost toggle around a place; undefined outside every toggle. */
function detailsDepth($pos: ResolvedPos): number | undefined {
  for (let depth = $pos.depth; depth > 0; depth -= 1) {
    if ($pos.node(depth).type.name === DETAILS) return depth;
  }
  return undefined;
}

/** The paragraph of the caret, where a toggle may stand in its place; its depth. */
function paragraphForToggle(state: EditorState): number | undefined {
  const { $from, $to } = state.selection;
  const depth = $from.depth;
  if (depth < 1 || !$from.sameParent($to) || $from.parent.type.name !== 'paragraph')
    return undefined;
  return takesDocumentBlocks($from.node(depth - 1)) ? depth : undefined;
}

function setDetails(state: EditorState, tr: Transaction): boolean {
  const depth = paragraphForToggle(state);
  if (depth === undefined) return false;
  const { schema } = state;
  const { $from } = state.selection;
  const line = $from.parent;
  const at = $from.before(depth);
  const toggle = schema.node(DETAILS, null, [
    schema.node(DETAILS_SUMMARY, null, line.content),
    schema.node(DETAILS_CONTENT, null, [schema.node('paragraph')]),
  ]);
  tr.replaceWith(at, at + line.nodeSize, toggle);
  tr.setSelection(TextSelection.create(tr.doc, at + 2 + $from.parentOffset));
  openAt(tr, at);
  return true;
}

/** The lines a toggle is made of again: its summary, then its content, if it says anything. */
function linesOf(state: EditorState, toggle: DocumentNode): DocumentNode[] {
  const summary = toggle.child(0);
  const content = toggle.child(1);
  const line = state.schema.node('paragraph', null, summary.content);
  const only = content.childCount === 1 ? content.firstChild : null;
  const says = !(only?.type.name === 'paragraph' && only.content.size === 0);
  const blocks: DocumentNode[] = [];
  if (says) content.forEach((block) => blocks.push(block));
  return [line, ...blocks];
}

function unsetDetails(state: EditorState, tr: Transaction): boolean {
  const { $from } = state.selection;
  const depth = detailsDepth($from);
  if (depth === undefined) return false;
  const toggle = $from.node(depth);
  const at = $from.before(depth);
  const summary = toggle.child(0);
  const lines = linesOf(state, toggle);
  // The caret keeps its place in the summary, which is the first line now, or in the content.
  const contentStart = at + 1 + summary.nodeSize + 1;
  const caret =
    $from.pos < contentStart
      ? at + 1 + ($from.pos - (at + 2))
      : at + summary.content.size + 2 + ($from.pos - contentStart);
  tr.replaceWith(at, at + toggle.nodeSize, Fragment.from(lines));
  const size = lines.reduce((sum, line) => sum + line.nodeSize, 0);
  const inside = Math.min(Math.max(caret, at + 1), at + size - 1);
  tr.setSelection(TextSelection.near(tr.doc.resolve(inside)));
  return true;
}

/** Whether a toggle says nothing: an empty summary over one empty line. */
function isBlank(toggle: DocumentNode): boolean {
  const only = toggle.child(1).childCount === 1 ? toggle.child(1).firstChild : null;
  return (
    toggle.child(0).content.size === 0 && only?.type.name === 'paragraph' && only.content.size === 0
  );
}

/**
 * Enter in a summary: on in the content, the rest of the summary its first
 * line. In a toggle that says nothing it is a line again, as an empty list
 * entry is.
 */
function enterInSummary(state: EditorState, dispatch?: (tr: Transaction) => void): boolean {
  const { selection } = state;
  const { $from } = selection;
  if (!selection.empty || $from.parent.type.name !== DETAILS_SUMMARY) return false;
  if (!dispatch) return true;
  const tr = state.tr;
  if (isBlank($from.node(-1))) {
    unsetDetails(state, tr);
    dispatch(tr.scrollIntoView());
    return true;
  }
  openAt(tr, $from.before(-1));
  const rest = $from.parent.content.cut($from.parentOffset);
  const contentStart = $from.after() + 1;
  const first = state.doc.nodeAt(contentStart);
  const firstEmpty = first?.type.name === 'paragraph' && first.content.size === 0;
  if (rest.size === 0 && firstEmpty) {
    tr.setSelection(TextSelection.create(tr.doc, contentStart + 1));
  } else {
    tr.delete($from.pos, $from.end());
    const start = tr.mapping.map(contentStart);
    tr.insert(start, state.schema.node('paragraph', null, rest));
    tr.setSelection(TextSelection.create(tr.doc, start + 1));
  }
  dispatch(tr.scrollIntoView());
  return true;
}

/** Backspace at the start of a summary: the toggle made lines again. */
function backspaceInSummary(state: EditorState, dispatch?: (tr: Transaction) => void): boolean {
  const { selection } = state;
  const { $from } = selection;
  if (!selection.empty || $from.parent.type.name !== DETAILS_SUMMARY || $from.parentOffset > 0)
    return false;
  if (!dispatch) return true;
  const tr = state.tr;
  unsetDetails(state, tr);
  dispatch(tr.scrollIntoView());
  return true;
}

/** The content around a line at the caret's start that is its first, and where the line is. */
function firstLineOfContent($from: ResolvedPos): boolean {
  return (
    $from.depth >= 2 &&
    $from.parentOffset === 0 &&
    $from.node(-1).type.name === DETAILS_CONTENT &&
    $from.index(-1) === 0
  );
}

/** Backspace at the start of what a toggle hides: back to the end of its summary. */
function backspaceInContent(state: EditorState, dispatch?: (tr: Transaction) => void): boolean {
  const { selection } = state;
  const { $from } = selection;
  if (!selection.empty || !$from.parent.isTextblock || !firstLineOfContent($from)) return false;
  if (!dispatch) return true;
  const tr = state.tr;
  const content = $from.node(-1);
  const summaryEnd = $from.before(-1) - 1;
  if ($from.parent.content.size === 0 && content.childCount > 1)
    tr.delete($from.before(), $from.after());
  tr.setSelection(TextSelection.create(tr.doc, summaryEnd));
  dispatch(tr.scrollIntoView());
  return true;
}

/** Enter on the empty last line of what a toggle hides: out of the toggle. */
function enterOnLastLine(state: EditorState, dispatch?: (tr: Transaction) => void): boolean {
  const { selection } = state;
  const { $from } = selection;
  const line = $from.parent;
  if (!selection.empty || line.type.name !== 'paragraph' || line.content.size > 0) return false;
  if ($from.depth < 3 || $from.node(-1).type.name !== DETAILS_CONTENT) return false;
  const content = $from.node(-1);
  if ($from.index(-1) !== content.childCount - 1) return false;
  if (!dispatch) return true;
  const tr = state.tr;
  if (content.childCount > 1) tr.delete($from.before(), $from.after());
  const after = tr.mapping.map($from.after(-2));
  tr.insert(after, state.schema.node('paragraph'));
  tr.setSelection(TextSelection.create(tr.doc, after + 1));
  dispatch(tr.scrollIntoView());
  return true;
}

/** Enter in a toggle: on in what it hides, or out of it. */
const enter = (state: EditorState, dispatch?: (tr: Transaction) => void): boolean =>
  enterInSummary(state, dispatch) || enterOnLastLine(state, dispatch);

/** Whether the page is Chrome on Android, by its name, as ProseMirror tells (its browser.ts). */
const isAndroidChrome = (page: Document): boolean => {
  const agent = page.defaultView?.navigator.userAgent ?? '';
  return /Android \d/.test(agent) && /Chrome\/\d/.test(agent);
};

/** Enter as a new line the page was asked for (see above), where Enter in a toggle is no split. */
const newLineAsEnter = (): Plugin =>
  new Plugin({
    props: {
      handleDOMEvents: {
        beforeinput: (view, event) => {
          if (event.inputType !== 'insertParagraph' || view.composing) return false;
          if (!isAndroidChrome(view.dom.ownerDocument)) return false;
          const dispatch = (tr: Transaction): void => {
            view.dispatch(tr);
          };
          if (!enter(view.state, dispatch)) return false;
          event.preventDefault();
          return true;
        },
      },
    },
  });

export const Details = Node.create({
  name: DETAILS,
  group: DOCUMENT_ONLY,
  content: `${DETAILS_SUMMARY} ${DETAILS_CONTENT}`,
  defining: true,
  isolating: true,
  allowGapCursor: false,

  parseHTML() {
    return [{ tag: 'details' }, { tag: 'div[data-details]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      'div',
      mergeAttributes(HTMLAttributes, { 'data-details': '', class: 'doc-details' }),
      0,
    ];
  },

  addNodeView() {
    return detailsView;
  },

  addCommands() {
    return {
      setDetails:
        () =>
        ({ state, tr, dispatch }) =>
          dispatch ? setDetails(state, tr) : paragraphForToggle(state) !== undefined,
      unsetDetails:
        () =>
        ({ state, tr, dispatch }) =>
          dispatch ? unsetDetails(state, tr) : detailsDepth(state.selection.$from) !== undefined,
      toggleDetails:
        () =>
        ({ commands, state }) =>
          detailsDepth(state.selection.$from) === undefined
            ? commands.setDetails()
            : commands.unsetDetails(),
    };
  },

  addKeyboardShortcuts(): Record<string, KeyboardShortcutCommand> {
    return {
      Enter: ({ editor }) => enter(editor.state, editor.view.dispatch),
      Backspace: ({ editor }) =>
        backspaceInSummary(editor.state, editor.view.dispatch) ||
        backspaceInContent(editor.state, editor.view.dispatch),
    };
  },

  addProseMirrorPlugins() {
    return [detailsOpenPlugin(), newLineAsEnter()];
  },
});

/** The line a toggle always shows: text and what stands in a line, nothing more. */
export const DetailsSummary = Node.create({
  name: DETAILS_SUMMARY,
  content: 'inline*',
  defining: true,
  isolating: true,
  selectable: false,

  parseHTML() {
    return [{ tag: 'summary' }, { tag: 'div[data-details-summary]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      'div',
      mergeAttributes(HTMLAttributes, { 'data-details-summary': '', class: 'doc-details-summary' }),
      0,
    ];
  },
});

/** What a toggle shows while it is open: what the document holds. */
export const DetailsContent = Node.create({
  name: DETAILS_CONTENT,
  content: `(block | ${DOCUMENT_ONLY})+`,
  defining: true,
  selectable: false,

  parseHTML() {
    return [{ tag: 'div[data-details-content]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      'div',
      mergeAttributes(HTMLAttributes, { 'data-details-content': '', class: 'doc-details-content' }),
      0,
    ];
  },
});
