/**
 * The board view of a body: a view of a board of the workspace, shown live
 * in the body, as wide as the page (`schema.ts`). The document holds the
 * view's identifier and nothing else; what the view shows (its board, its
 * cards, its filter, sort, properties and grouping) is read where it is
 * shown, for whoever reads, as a reference's title is.
 *
 * This module draws only the place of one, in the editor (a node view);
 * where a body is only read, board-view-read.ts draws it, with nothing of
 * the editor. What is drawn into the place comes from `BoardViewHost.mount`,
 * which the app gives, so that nothing here knows of boards or of their
 * state.
 *
 * A board view stands where a table does: in the document and in what a
 * toggle hides, not in a list, a quote, a callout or a cell; and a body
 * shows ten at most, which no change of the reader's goes past, however it
 * was made (a paste of blocks, a drop): one past them already is the
 * server's to refuse. Only the body of a card, a page or a unit has one: the
 * editor of a comment or of a diary's entry is given `allowed: false`, and
 * makes none of what is pasted, nor offers one.
 *
 * One is put in by the slash menu (`boardViewEntry`: the app asks for a
 * board, then a view) or by pasting the address of a view, which is a link
 * until the reader takes it as a board view (board-view-paste.ts).
 *
 * In the editor the place is reached with the arrow keys, which select the
 * block, and its frame is the block's to press, select and drag; what the
 * view draws inside is its own, and the editor takes no event of it.
 */
import { mergeAttributes, Node, type Editor } from '@tiptap/core';
import type { Slice } from '@tiptap/pm/model';
import { NodeSelection, Plugin, type EditorState, type Transaction } from '@tiptap/pm/state';

import { m } from '$lib/i18n/paraglide/messages.js';

import { boardViewPaste } from './board-view-paste.ts';
import { BOARD_VIEW, boardViewCount, boardViewPlace, canPutBoardView } from './board-view-place.ts';
import { MAX_BOARD_VIEWS } from './schema.ts';
import { isBlockId } from './schema-rules.ts';
import { fromShared } from './shared-place.ts';
import type { SlashEntry } from './slash-menu.ts';
import type { ChoiceList } from './suggestion-list.ts';
import {
  BOARD_VIEW_ATTRIBUTE,
  drawBoardViewPlace,
  drawView,
  mountBoardViews,
  VIEW_ID_ATTRIBUTE,
  type BoardViewHost,
  type MountBoardView,
} from './board-view-read.ts';
import { DOCUMENT_ONLY } from './table.ts';

import './board-view.css';

export { BOARD_VIEW, boardViewCount, canPutBoardView };

/** Whether a step of `tr` brings a board view in: none does where the reader types. */
function bringsBoardView(tr: Transaction): boolean {
  return tr.steps.some((step) => {
    const { slice } = step as { slice?: Slice };
    let found = false;
    slice?.content.descendants((node) => {
      if (node.type.name === BOARD_VIEW) found = true;
      return !found && !node.isTextblock;
    });
    return found;
  });
}

/**
 * No change of the reader's takes a body past `MAX_BOARD_VIEWS` (one past it
 * already is the server's to refuse); another's, or the shared undo, is the
 * shared document's.
 */
const withinBoardViews = (): Plugin =>
  new Plugin({
    filterTransaction(tr: Transaction, state: EditorState) {
      if (!tr.docChanged || fromShared(tr) || !bringsBoardView(tr)) return true;
      const count = boardViewCount(tr.doc);
      return count <= MAX_BOARD_VIEWS || count <= boardViewCount(state.doc);
    },
  });
export {
  BOARD_VIEW_ATTRIBUTE,
  drawBoardViewPlace,
  mountBoardViews,
  VIEW_ID_ATTRIBUTE,
  type BoardViewHost,
  type MountBoardView,
};

export interface BoardViewOptions extends Partial<BoardViewHost> {
  /**
   * Whether a board view may be written here; true unless given. The editor
   * of a comment and of a diary's entry is given false: a board view pasted
   * there is no block, and none is offered.
   */
  allowed?: boolean;
  /** Makes the list a pasted address of a view offers its two ways in. */
  choices?: () => ChoiceList;
}

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    boardView: {
      /**
       * Puts a board view of the view `viewId` where a block may stand: in
       * place of the empty line the selection is on, or after its block.
       * False where none may stand, or where the body shows ten.
       */
      setBoardView: (viewId: string) => ReturnType;
    };
  }
}

/** Puts a board view into `tr` where its selection says; false where none may go. */
function putBoardView(tr: Transaction, viewId: string, dispatch: boolean): boolean {
  const type = tr.doc.type.schema.nodes[BOARD_VIEW];
  const place = boardViewPlace(tr.selection);
  if (!type || !place || boardViewCount(tr.doc) >= MAX_BOARD_VIEWS) return false;
  if (dispatch) {
    tr.replaceWith(place.from, place.to, type.create({ viewId }));
    tr.setSelection(NodeSelection.create(tr.doc, place.from)).scrollIntoView();
  }
  return true;
}

/**
 * The entry of the slash menu that asks the reader for a view and puts a
 * board view of it where the selection is then; none where one cannot go.
 */
export function boardViewEntry(choose: BoardViewHost['choose']): (editor: Editor) => SlashEntry[] {
  return (editor) =>
    canPutBoardView(editor.state.doc, editor.state.selection)
      ? [
          {
            value: 'board-view',
            label: m.editor_slash_board_view(),
            words: ['board view', 'embed', 'view', '임베드', '보드', '뷰'],
            run: (chosen) => {
              void choose().then((viewId) => {
                if (viewId && !chosen.isDestroyed)
                  chosen.chain().focus().setBoardView(viewId).run();
              });
              return true;
            },
          },
        ]
      : [];
}

export const BoardView = Node.create<BoardViewOptions>({
  name: BOARD_VIEW,
  group: DOCUMENT_ONLY,
  atom: true,
  draggable: true,
  selectable: true,

  addOptions() {
    return { allowed: true };
  },

  addAttributes() {
    return {
      viewId: {
        default: null,
        parseHTML: (element: HTMLElement) => element.getAttribute(VIEW_ID_ATTRIBUTE),
        renderHTML: (attrs: { viewId?: string | null }) => ({ [VIEW_ID_ATTRIBUTE]: attrs.viewId }),
      },
    };
  },

  parseHTML() {
    if (this.options.allowed === false) return [];
    return [
      {
        tag: `div[${BOARD_VIEW_ATTRIBUTE}][${VIEW_ID_ATTRIBUTE}]`,
        getAttrs: (element: HTMLElement) =>
          isBlockId(element.getAttribute(VIEW_ID_ATTRIBUTE)) ? null : false,
      },
    ];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      'div',
      mergeAttributes(HTMLAttributes, { [BOARD_VIEW_ATTRIBUTE]: '', class: 'doc-board-view' }),
    ];
  },

  addNodeView() {
    const mount = this.options.mount;
    return ({ node, HTMLAttributes }) => {
      const dom = document.createElement('div');
      const attributes = mergeAttributes(HTMLAttributes, {
        [BOARD_VIEW_ATTRIBUTE]: '',
        class: 'doc-board-view',
      });
      for (const [name, value] of Object.entries(attributes)) {
        if (value !== null && value !== undefined) dom.setAttribute(name, String(value));
      }
      dom.contentEditable = 'false';
      const place = document.createElement('div');
      place.className = 'doc-board-view-place';
      dom.append(place);
      const stop = drawView(place, node.attrs.viewId, mount);
      return {
        dom,
        // What the view draws is its own: its presses, keys and typing are not the editor's.
        stopEvent: (event) => event.target instanceof Element && place.contains(event.target),
        // What is drawn into the place is no change of the document.
        ignoreMutation: () => true,
        // Another view is another block, drawn anew.
        update: (updated) =>
          updated.type === node.type && updated.attrs.viewId === node.attrs.viewId,
        destroy: stop,
      };
    };
  },

  addCommands() {
    return {
      setBoardView:
        (viewId) =>
        ({ tr, dispatch }) =>
          putBoardView(tr, viewId, dispatch !== undefined),
    };
  },

  addProseMirrorPlugins() {
    const { allowed, viewAt, seen, choices } = this.options;
    if (allowed === false || !viewAt || !seen || !choices) return [withinBoardViews()];
    return [withinBoardViews(), boardViewPaste(this.editor, { viewAt, seen, choices })];
  },
});
