/**
 * The address of a view of a board, pasted into a body that may show board
 * views (board-view.ts): it goes in as the text it is, a link, and once it is
 * known that the reader may see the view, a list under it offers to keep it
 * so ("as a link") or to put a board view of the view in its place ("as an
 * embed"). The list is the one an autocomplete shows (suggestion-list.ts):
 * the arrows and Enter choose, Escape keeps the link.
 *
 * It is offered only where a board view may stand and the body shows fewer
 * than ten, over no selection and not in code. Anything done meanwhile (a
 * letter typed, the caret moved, another's change of the text) keeps the
 * link and takes the list away: what would be replaced must still be the
 * address, where it was pasted; and the link stays when a board view can no
 * longer be put there as it is chosen (another filled the body meanwhile).
 */
import type { Editor } from '@tiptap/core';
import { Plugin, PluginKey, type EditorState } from '@tiptap/pm/state';
import type { EditorView } from '@tiptap/pm/view';

import { m } from '$lib/i18n/paraglide/messages.js';

import { canPutBoardView } from './board-view-place.ts';
import { isBlockId } from './schema-rules.ts';
import type { ChoiceList } from './suggestion-list.ts';

export interface BoardViewPasteOptions {
  viewAt: (address: string) => string | undefined;
  seen: (viewId: string) => Promise<boolean>;
  /** Makes the list the two ways are offered in. */
  choices: () => ChoiceList;
}

/** A pasted address of a view, where it is now. */
interface Pasted {
  key: number;
  from: number;
  to: number;
  text: string;
  viewId: string;
}

type PasteMeta = { add: Pasted } | { done: number };

const pasteKey = new PluginKey<Pasted | undefined>('boardViewPaste');

/** Whether the selection is in code: there an address is text. */
function inCode(state: EditorState): boolean {
  const { $from } = state.selection;
  return Boolean($from.parent.type.spec.code) || $from.marks().some((mark) => mark.type.spec.code);
}

/** Whether the pasted address is still the text where it was pasted, the caret after it. */
function standing(state: EditorState, pasted: Pasted): boolean {
  const { from, to, text } = pasted;
  return (
    from < to &&
    to <= state.doc.content.size &&
    state.doc.textBetween(from, to) === text &&
    state.selection.empty &&
    state.selection.from === to
  );
}

const LINK = 'link';
const EMBED = 'embed';

/** The list of the two ways, open under one pasted address at a time. */
class Offer {
  #open: { pasted: Pasted; list: ChoiceList } | undefined;
  readonly #editor: Editor;
  readonly #choices: () => ChoiceList;

  constructor(editor: Editor, choices: () => ChoiceList) {
    this.#editor = editor;
    this.#choices = choices;
  }

  /** The list open now. */
  get list(): ChoiceList | undefined {
    return this.#open?.list;
  }

  /** Whether the list open is of an address that no longer stands where it was pasted. */
  stale(state: EditorState): boolean {
    return this.#open !== undefined && !standing(state, this.#open.pasted);
  }

  /** Takes the list away and forgets the paste: the address stays a link. */
  close(view: EditorView): void {
    if (!this.#open) return;
    const { pasted, list } = this.#open;
    this.#open = undefined;
    list.close();
    if (!view.isDestroyed && pasteKey.getState(view.state)?.key === pasted.key) {
      view.dispatch(view.state.tr.setMeta(pasteKey, { done: pasted.key } satisfies PasteMeta));
    }
  }

  /** Takes the list away with nothing more: the editor is going. */
  drop(): void {
    this.#open?.list.close();
    this.#open = undefined;
  }

  /** Opens the list under the address, if it still stands and is still the last paste. */
  show(view: EditorView, pasted: Pasted): void {
    if (view.isDestroyed || this.#open || pasteKey.getState(view.state)?.key !== pasted.key) return;
    if (!standing(view.state, pasted)) return;
    const list = this.#choices();
    this.#open = { pasted, list };
    const at = view.coordsAtPos(pasted.to);
    list.open(
      view,
      [
        { value: LINK, label: m.editor_board_view_as_link() },
        { value: EMBED, label: m.editor_board_view_as_embed() },
      ],
      (entry) => {
        this.#take(view, pasted, entry.value);
      },
      new DOMRect(at.left, at.top, 0, at.bottom - at.top),
    );
  }

  #take(view: EditorView, pasted: Pasted, way: string): void {
    this.close(view);
    if (way !== EMBED || !standing(view.state, pasted)) return;
    // Another's board views may have filled the body meanwhile: the address then stays,
    // since the chain would take it away and put nothing in its place.
    if (!canPutBoardView(view.state.doc, view.state.selection)) return;
    this.#editor
      .chain()
      .focus()
      .deleteRange({ from: pasted.from, to: pasted.to })
      .setBoardView(pasted.viewId)
      .run();
  }
}

/** The view a paste names, where it may be offered as a board view; nothing otherwise. */
function pastedView(
  view: EditorView,
  event: ClipboardEvent,
  viewAt: BoardViewPasteOptions['viewAt'],
): { text: string; viewId: string } | undefined {
  const text = event.clipboardData?.getData('text/plain').trim() ?? '';
  if (text === '' || /\s/.test(text) || !view.state.selection.empty || inCode(view.state)) {
    return undefined;
  }
  const viewId = viewAt(text);
  if (viewId === undefined || !isBlockId(viewId)) return undefined;
  return canPutBoardView(view.state.doc, view.state.selection) ? { text, viewId } : undefined;
}

export function boardViewPaste(editor: Editor, options: BoardViewPasteOptions): Plugin {
  let pastes = 0;
  const offer = new Offer(editor, options.choices);

  return new Plugin<Pasted | undefined>({
    key: pasteKey,
    state: {
      init: () => undefined,
      apply(tr, held) {
        const meta = tr.getMeta(pasteKey) as PasteMeta | undefined;
        if (meta && 'add' in meta) return meta.add;
        if (!held || (meta && 'done' in meta && meta.done === held.key)) return undefined;
        if (!tr.docChanged) return held;
        return { ...held, from: tr.mapping.map(held.from, 1), to: tr.mapping.map(held.to, -1) };
      },
    },
    view: () => ({
      update: (view) => {
        // Whatever moved the caret or changed the address keeps the link.
        if (offer.stale(view.state)) offer.close(view);
      },
      destroy: () => {
        offer.drop();
      },
    }),
    props: {
      handlePaste: (view, event) => {
        const named = pastedView(view, event, options.viewAt);
        if (!named) return false;
        offer.close(view);
        const from = view.state.selection.from;
        // Pasted with no clipboard of its own, it does not come here again.
        view.pasteText(named.text);
        pastes += 1;
        const pasted: Pasted = { key: pastes, from, to: view.state.selection.from, ...named };
        view.dispatch(view.state.tr.setMeta(pasteKey, { add: pasted } satisfies PasteMeta));
        void options
          .seen(named.viewId)
          .catch(() => false)
          .then((seen) => {
            if (seen) offer.show(view, pasted);
          });
        return true;
      },
      handleKeyDown: (view, event) => {
        const list = offer.list;
        if (!list) return false;
        const handled = list.keyDown(event);
        // Escape closes the list and keeps the link.
        if (event.key === 'Escape' && handled) offer.close(view);
        return handled;
      },
      handleDOMEvents: {
        blur: (view) => {
          offer.close(view);
          return false;
        },
      },
    },
  });
}
