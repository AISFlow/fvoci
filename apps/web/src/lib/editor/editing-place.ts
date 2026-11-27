/**
 * The editing place: where the source of a diagram or of a block formula is
 * changed, opened right under the picture and inside the flow of the body.
 *
 * At most one place is open in an editor. Which one is a state of the screen,
 * kept by this plugin beside the document and never in it: opening and
 * closing a place changes no node, no attribute and no text, and so nothing
 * that is stored. The position of the open block is mapped through every
 * change; a change that takes the block away closes the place.
 *
 * Closed, a diagram is its picture and a block formula its drawing. Moving
 * through the body with the arrow keys selects such a block as a whole
 * (Enter then opens it) and never puts the cursor into the hidden source.
 *
 * - Opens: a press on the picture (the node views), Enter while the block is
 *   selected, and a new diagram (an empty one just made) opens by itself.
 * - Closes: Escape, ⌘Enter on Apple devices and Ctrl+Enter elsewhere, a press anywhere else, the
 *   focus going elsewhere, or the cursor leaving the block. Closed by a key,
 *   the block is selected again and the editor has the focus.
 */
import { Extension } from '@tiptap/core';
import {
  NodeSelection,
  Plugin,
  Selection,
  TextSelection,
  type EditorState,
} from '@tiptap/pm/state';
import { Decoration, DecorationSet, type EditorView } from '@tiptap/pm/view';

import { isComposing, shortcutLabel } from '$lib/keys.ts';
import { m } from '$lib/i18n/paraglide/messages.js';

import { diagramKind } from './diagram-kind.ts';
import { lockMathEditing } from './math-editing.ts';
import { mayWrite, onShared, sayDropped } from './shared-place.ts';
import {
  closedDiagramAt,
  closedOnPurpose,
  closePlace,
  closesPlace,
  heldWritten,
  holdAgain,
  isDiagramNode,
  keepOutOfClosed,
  keepPlaceSelected,
  nextPlace,
  openPlace,
  placeKey,
  placeOf,
  type Place,
} from './place-state.ts';

export {
  closePlace,
  closesPlace,
  heldWritten,
  isDiagramNode,
  openNewFormula,
  openPlace,
  placeIn,
  placeKey,
  placeOf,
  type Draft,
  type NewFormulaPanel,
  type Place,
  type PlaceKind,
} from './place-state.ts';

const ARROWS: Record<string, { dir: -1 | 1; side: 'up' | 'down' | 'left' | 'right' }> = {
  ArrowUp: { dir: -1, side: 'up' },
  ArrowDown: { dir: 1, side: 'down' },
  ArrowLeft: { dir: -1, side: 'left' },
  ArrowRight: { dir: 1, side: 'right' },
};

/**
 * An arrow key at the edge of a line that would go into a closed diagram
 * selects the diagram. (The browser would skip its hidden source.)
 */
function arrowOntoDiagram(view: EditorView, event: KeyboardEvent): boolean {
  const arrow = ARROWS[event.key];
  if (!arrow || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return false;
  const { selection, doc } = view.state;
  if (!(selection instanceof TextSelection) || !selection.empty) return false;
  const { $head } = selection;
  if (!$head.parent.inlineContent || $head.depth === 0) return false;
  if (!view.endOfTextblock(arrow.side)) return false;
  const $start = doc.resolve(arrow.dir > 0 ? $head.after() : $head.before());
  const found = Selection.findFrom($start, arrow.dir);
  const pos = found ? closedDiagramAt(view.state, found.$from) : undefined;
  if (pos === undefined) return false;
  view.dispatch(view.state.tr.setSelection(NodeSelection.create(doc, pos)).scrollIntoView());
  return true;
}

/** Chrome on Android leaves Enter to DOM reconciliation (`handleKeyDown`) so Gboard composition is not disrupted. */
function isAndroidChrome(): boolean {
  const agent = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  return agent.includes('Android') && agent.includes('Chrome/');
}

/** Whether `event` inserts a newline into an open editable diagram's source. */
function isDiagramEnter(view: EditorView, event: KeyboardEvent): boolean {
  return (
    placeOf(view.state)?.kind === 'diagram' &&
    event.key === 'Enter' &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey &&
    view.editable
  );
}

/**
 * The keys that open, close and break lines in a place, heard before the
 * editor's own handling: the editor leaves Enter on Android to the change it
 * makes in the page (which a selected diagram does not make) and on iOS lets
 * native Enter split the source's `<pre>` and rebuild the diagram view.
 */
function placeKeys(view: EditorView, event: KeyboardEvent): boolean {
  if (isComposing(event) || view.composing) return false;
  const place = placeOf(view.state);
  const { selection } = view.state;
  if (place && closesPlace(event)) {
    // The key belongs to the place: it must not also close what the editor is in.
    event.stopPropagation();
    closePlace(view, true);
  } else if (!isAndroidChrome() && isDiagramEnter(view, event)) {
    // A new line, always (Shift+Enter too): a source may have empty lines, and three Enters
    // do not leave it.
    view.dispatch(view.state.tr.insertText('\n').scrollIntoView());
  } else if (
    !place &&
    event.key === 'Enter' &&
    !event.shiftKey &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey &&
    selection instanceof NodeSelection &&
    view.editable
  ) {
    if (!openPlace(view, selection.from)) return false;
  } else return false;
  event.preventDefault();
  return true;
}

function handleKeyDown(view: EditorView, event: KeyboardEvent): boolean {
  if (isComposing(event)) return false;
  if (isDiagramEnter(view, event)) {
    // Fallback for synthetic Enter dispatched from DOM reconciliation (e.g. Android virtual keyboard).
    view.dispatch(view.state.tr.insertText('\n').scrollIntoView());
    return true;
  }
  return arrowOntoDiagram(view, event) || keyIntoDiagram(view, event);
}

/**
 * Backspace at the start of the line under a closed diagram, or Delete at the
 * end of the line over it, selects the diagram, as for any block that is a
 * picture: it does not join that line's text to the hidden source.
 */
function keyIntoDiagram(view: EditorView, event: KeyboardEvent): boolean {
  const back = event.key === 'Backspace';
  if (!back && event.key !== 'Delete') return false;
  if (event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return false;
  const { selection, doc } = view.state;
  if (!(selection instanceof TextSelection) || !selection.empty) return false;
  const { $head } = selection;
  if (!$head.parent.isTextblock || $head.depth === 0) return false;
  const atEdge = back ? $head.parentOffset === 0 : $head.parentOffset === $head.parent.content.size;
  if (!atEdge) return false;
  const $edge = doc.resolve(back ? $head.before() : $head.after());
  const next = back ? $edge.nodeBefore : $edge.nodeAfter;
  if (!next || !isDiagramNode(next)) return false;
  const pos = back ? $edge.pos - next.nodeSize : $edge.pos;
  if (placeOf(view.state)?.pos === pos) return false;
  view.dispatch(view.state.tr.setSelection(NodeSelection.create(doc, pos)).scrollIntoView());
  return true;
}

/** The element a press or the focus has to stay inside for the place to stay open. */
function holderOf(view: EditorView, place: Place): Node | null {
  return place.panel?.dom ?? view.nodeDOM(place.pos);
}

/** What a screen reader is told when a place opens, or a block is selected. */
function announcement(view: EditorView, before: EditorState): string | undefined {
  const { state } = view;
  // Where nothing can be edited there is nothing to offer.
  if (!view.editable) return undefined;
  const place = placeOf(state);
  if (place) return placeOf(before) ? undefined : m.editor_place_opened();
  const { selection } = state;
  if (!(selection instanceof NodeSelection)) return undefined;
  if (before.selection.eq(selection) && !placeOf(before)) return undefined;
  if (isDiagramNode(selection.node))
    return m.editor_diagram_selected({ kind: diagramKind(selection.node.textContent) });
  if (selection.node.type.name === 'blockMath')
    return m.editor_math_selected({ latex: String(selection.node.attrs.latex) });
  return undefined;
}

/** What the plugin does in the page: the focus of a place that opens, closing from outside, the live region. */
function placeView(view: EditorView) {
  const page = view.dom.ownerDocument;
  const live = page.createElement('div');
  live.setAttribute('class', 'doc-announce');
  // Not a `status`: the editor's own line of status is that (BodyEditor.svelte).
  live.setAttribute('aria-live', 'polite');
  view.dom.after(live);
  const outside = (target: EventTarget | null): boolean => {
    const place = placeOf(view.state);
    if (!place || !(target instanceof Node)) return false;
    return !holderOf(view, place)?.contains(target);
  };
  // A press, not a touch: a finger that only scrolls the page leaves the place open.
  const onClick = (event: Event): void => {
    if (outside(event.target) && !view.composing) closePlace(view, false);
  };
  const onFocusIn = (event: Event): void => {
    // The editor itself is "elsewhere" only for a formula, whose field has the focus; for a
    // diagram a press in the editor moves the selection, which closes.
    const place = placeOf(view.state);
    const away = event.target !== view.dom || (place !== null && place.kind !== 'diagram');
    if (outside(event.target) && away && !view.composing) closePlace(view, false);
  };
  page.addEventListener('click', onClick, true);
  page.addEventListener('focusin', onFocusIn);
  const stopHolding = onShared(view, () => {
    holdAgain(view.state);
  });
  return {
    update: (_: EditorView, before: EditorState) => {
      // A written field gone with its block (deleted, or made anew by somebody else): said.
      const was = placeOf(before);
      if (was && !placeOf(view.state) && heldWritten(was) && !closedOnPurpose(was)) {
        sayDropped(view, 'gone');
      }
      // Nothing of the document changed: the shared document has it all already.
      if (view.state.doc === before.doc) holdAgain(view.state);
      const place = placeOf(view.state);
      // A formula's box is locked while the editor is read-only: nothing more is typed into it.
      lockMathEditing(view, !view.editable);
      if (place && !view.editable) {
        // The editor became read-only with a place open. Only for a moment (the connection
        // away): what the field holds is kept as if closed on purpose, and the room sends it
        // when it can, or says it was lost. For a reader who may only read: nothing is
        // written, and that is said. Not from inside this update.
        queueMicrotask(() => {
          if (view.isDestroyed) return;
          const dropping = !mayWrite(view);
          if (dropping && heldWritten(place)) sayDropped(view, 'refused');
          closePlace(view, false, dropping);
        });
        return;
      }
      const said = announcement(view, before);
      if (said !== undefined) live.textContent = said;
      if (!place || placeOf(before)) return;
      // Just opened: the field of a formula takes the focus (a diagram's source is the editor's).
      if (place.panel) place.panel.focus();
      else if (place.kind === 'formula') {
        const holder = holderOf(view, place);
        if (holder instanceof HTMLElement) holder.querySelector('textarea')?.focus();
      }
    },
    destroy: () => {
      stopHolding();
      page.removeEventListener('click', onClick, true);
      page.removeEventListener('focusin', onFocusIn);
      live.remove();
    },
  };
}

export const EditingPlace = Extension.create({
  name: 'editingPlace',
  // Before the keys of the blocks themselves: Enter on a selected block, Escape in a source.
  priority: 1000,

  addProseMirrorPlugins() {
    return [
      new Plugin<Place | null>({
        key: placeKey,
        state: {
          init: () => null,
          apply: (tr, value, before, state) => nextPlace(value, tr, before, state),
        },
        appendTransaction: (transactions, before, state) =>
          keepPlaceSelected(transactions, state) ?? keepOutOfClosed(transactions, before, state),
        props: {
          handleDOMEvents: { keydown: placeKeys },
          handleKeyDown,
          // Text typed while a closed diagram or block formula is selected does not take its place: passing
          // over one with the arrows is not one key away from losing it.
          handleTextInput: (view) => {
            const { selection } = view.state;
            return (
              selection instanceof NodeSelection &&
              (isDiagramNode(selection.node) || selection.node.type.name === 'blockMath')
            );
          },
          decorations: (state) => {
            const place = placeOf(state);
            if (!place) return null;
            if (place.panel) {
              // On a line somebody else wrote in meanwhile, after its text: where it will go.
              const line = state.doc.nodeAt(place.pos);
              const after = place.lineTaken === true && line !== null;
              const at = after ? place.pos + line.nodeSize - 1 : place.pos + 1;
              return DecorationSet.create(state.doc, [
                Decoration.widget(at, place.panel.dom, {
                  side: after ? 1 : -1,
                  key: 'new-formula',
                  ignoreSelection: true,
                  stopEvent: () => true,
                }),
              ]);
            }
            const node = state.doc.nodeAt(place.pos);
            if (!node) return null;
            return DecorationSet.create(state.doc, [
              Decoration.node(place.pos, place.pos + node.nodeSize, {}, { place }),
            ]);
          },
        },
        view: placeView,
      }),
    ];
  },
});

/** The line under a source that is being changed: how to close, and a button that does. */
export function placeFooter(
  page: Document,
  done: () => void,
  hint = m.editor_place_hint({ shortcut: shortcutLabel('Enter') }),
): HTMLElement {
  const footer = page.createElement('div');
  footer.setAttribute('class', 'doc-place-footer');
  footer.setAttribute('contenteditable', 'false');
  const text = page.createElement('p');
  text.setAttribute('class', 'doc-place-hint');
  text.append(page.createTextNode(hint));
  const button = page.createElement('button');
  button.setAttribute('type', 'button');
  button.dataset.action = 'done';
  button.append(page.createTextNode(m.ui_done()));
  button.addEventListener('click', done);
  footer.append(text, button);
  return footer;
}

/**
 * The small "Edit" on a picture that is pointed at or selected: the same press
 * as on the picture, for the eye. Not a button of its own for the keys or a
 * screen reader, which the picture already serves (its name, Enter).
 */
export function placeEditButton(page: Document, press: () => void): HTMLElement {
  const button = page.createElement('span');
  button.setAttribute('class', 'doc-place-edit');
  button.setAttribute('contenteditable', 'false');
  button.setAttribute('aria-hidden', 'true');
  button.append(page.createTextNode(m.editor_place_edit()));
  button.addEventListener('click', (event) => {
    event.preventDefault();
    press();
  });
  return button;
}
