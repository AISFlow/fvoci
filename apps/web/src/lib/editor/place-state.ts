/**
 * The state of the editing place (editing-place.ts): which block's place is
 * open, kept beside the document and never in it, and the transactions that
 * open and close it. Closing a formula's place on purpose is what writes its
 * source; a place that only goes away writes nothing.
 */
import type { Node as DocumentNode, ResolvedPos } from '@tiptap/pm/model';
import { closeHistory } from '@tiptap/pm/history';
import {
  NodeSelection,
  PluginKey,
  Selection,
  TextSelection,
  type EditorState,
  type Transaction,
} from '@tiptap/pm/state';
import type { Decoration, EditorView } from '@tiptap/pm/view';

import { isComposing, primaryModifier } from '$lib/keys.ts';

import { DIAGRAM_LANGUAGE } from './schema.ts';
import { fromShared, holdNode, nodeNow, type HeldNode } from './shared-place.ts';

/** A diagram, a block formula, or a block formula that is still being written. */
export type PlaceKind = 'diagram' | 'formula' | 'new-formula';

/** What a block formula holds in its own field, which may differ from the document (empty). */
export interface Draft {
  /** The source as written, trimmed. Set by whoever holds the field. */
  value: () => string;
  /** Whether the field was written in since the place opened; kept with the place, not its view. */
  touched?: boolean;
}

/** The place of a formula that is not in the document yet, drawn on its empty line. */
export interface NewFormulaPanel extends Draft {
  dom: HTMLElement;
  focus: () => void;
}

export interface Place {
  kind: PlaceKind;
  /** Where the block starts: the diagram, the formula, or the empty line a new formula goes into. */
  pos: number;
  /**
   * The block in the shared document of a body edited together: a change
   * from there replaces the blocks it changed, which the mapping of `pos`
   * cannot follow (shared-place.ts). Held again by the place's view after
   * each change of the reader's (`holdAgain`), once it is in the shared
   * document; kept by every copy of the place.
   */
  anchor?: { held?: HeldNode; stale: boolean };
  /**
   * For a new formula: somebody else wrote into its empty line meanwhile.
   * The place stays, and the formula goes in after that line.
   */
  lineTaken?: boolean;
  /** For a new formula: its place, which the document does not hold yet. */
  panel?: NewFormulaPanel;
  /** For a formula: its field, set by its node view while the place is open. */
  draft?: Draft;
}

type Meta = { open: Place } | { close: true };

export const placeKey = new PluginKey<Place | null>('editingPlace');

export const isDiagramNode = (node: DocumentNode | null | undefined): boolean =>
  node?.type.name === 'codeBlock' && node.attrs.language === DIAGRAM_LANGUAGE;

const isEmptyParagraph = (node: DocumentNode | null | undefined): boolean =>
  node?.type.name === 'paragraph' && node.content.size === 0;

/** Whether the node at a place is still the kind of block the place is for. */
function fits(kind: PlaceKind, node: DocumentNode | null | undefined): boolean {
  if (kind === 'diagram') return isDiagramNode(node);
  if (kind === 'formula') return node?.type.name === 'blockMath';
  return isEmptyParagraph(node);
}

/** The place that is open, or null. */
export const placeOf = (state: EditorState): Place | null => placeKey.getState(state) ?? null;

/** Whether the selection belongs to the open place: inside its block, or on it. */
function selectionStays(place: Place, state: EditorState): boolean {
  const node = state.doc.nodeAt(place.pos);
  if (!node) return false;
  const { from, to, anchor } = state.selection;
  if (place.kind === 'formula')
    return state.selection instanceof NodeSelection && from === place.pos;
  const inside = (at: number): boolean => at > place.pos && at < place.pos + node.nodeSize;
  // A diagram's source stays in sight while a selection that starts in it is drawn out of it
  // (Shift and an arrow, a drag); a new line that leaves it is no new line.
  if (place.kind === 'diagram') return inside(anchor);
  return inside(from) && inside(to);
}

/** The diagram whose source the position is in, when that diagram is closed. */
export function closedDiagramAt(state: EditorState, $position: ResolvedPos): number | undefined {
  if (!isDiagramNode($position.parent) || $position.depth === 0) return undefined;
  const pos = $position.before();
  const place = placeOf(state);
  return place?.kind === 'diagram' && place.pos === pos ? undefined : pos;
}

export function nextPlace(
  value: Place | null,
  tr: Transaction,
  before: EditorState,
  state: EditorState,
): Place | null {
  const meta = tr.getMeta(placeKey) as Meta | undefined;
  if (meta && 'close' in meta) {
    if (value) closings.add(value);
    return null;
  }
  let place = meta?.open ?? value;
  if (!place) return null;
  const held = place.anchor?.held;
  if (meta) place = { ...place, anchor: { stale: true } };
  else if (held && fromShared(tr)) {
    // Somebody else's change, or the shared undo: the block is found again in the shared document.
    const now = nodeNow(before, held, state.doc);
    if (now === null) return null;
    if (now !== undefined && now !== place.pos) place = { ...place, pos: now };
  } else if (tr.docChanged) {
    const mapped = tr.mapping.mapResult(place.pos, 1);
    if (mapped.deleted) return null;
    if (place.anchor) place.anchor.stale = true;
    if (mapped.pos !== place.pos) place = { ...place, pos: mapped.pos };
  }
  const node = state.doc.nodeAt(place.pos);
  if (place.kind === 'new-formula' && fromShared(tr) && !isEmptyParagraph(node)) {
    if (node?.type.name === 'paragraph' && !place.lineTaken) place = { ...place, lineTaken: true };
  }
  const lineKept = place.lineTaken === true && node?.type.name === 'paragraph';
  if (!lineKept && !fits(place.kind, node)) return null;
  // A change from the room moves the selection by the binding's guess, not the reader's
  // (`keepPlaceSelected` puts it back).
  return fromShared(tr) || selectionStays(place, state) ? place : null;
}

/**
 * What a formula's place leaves in the document when it is closed on
 * purpose: what its field holds, as one step of undo of its own. A formula
 * closed empty is removed; a new one that was written goes in place of its
 * empty line. Gives the position of the formula, if there is one.
 */
function finishFormula(tr: Transaction, place: Place): number | undefined {
  const written = (place.panel ?? place.draft)?.value() ?? '';
  const node = tr.doc.nodeAt(place.pos);
  if (!node) return undefined;
  const { pos } = place;
  if (place.kind === 'formula') {
    if (written === String(node.attrs.latex)) return pos;
    closeHistory(tr);
    if (written === '') {
      tr.delete(pos, pos + node.nodeSize);
      return undefined;
    }
    tr.setNodeAttribute(pos, 'latex', written);
    return pos;
  }
  const formula =
    written === '' ? undefined : tr.doc.type.schema.nodes.blockMath?.create({ latex: written });
  if (!formula) return undefined;
  if (!isEmptyParagraph(node)) {
    // The line was written in by somebody else meanwhile: the formula goes after it.
    if (!place.lineTaken || node.type.name !== 'paragraph') return undefined;
    closeHistory(tr);
    const at = pos + node.nodeSize;
    tr.insert(at, formula);
    // Somewhere to go on writing when the formula ends what holds it.
    const paragraph = tr.doc.type.schema.nodes.paragraph?.create();
    if (tr.doc.resolve(at + formula.nodeSize).nodeAfter === null && paragraph) {
      tr.insert(at + formula.nodeSize, paragraph);
    }
    return at;
  }
  closeHistory(tr);
  tr.replaceWith(pos, pos + node.nodeSize, formula);
  // Somewhere to go on writing when the formula ends what holds it.
  if (tr.doc.resolve(pos + formula.nodeSize).nodeAfter === null) {
    const paragraph = tr.doc.type.schema.nodes.paragraph?.create();
    if (paragraph) tr.insert(pos + formula.nodeSize, paragraph);
  }
  return pos;
}

/**
 * Where an end of a selection goes that lies in the hidden source of a closed
 * diagram: past it, away from the other end, and past every closed diagram
 * that follows, to the first place for text; with none, the edge of the body.
 */
function pastClosed(state: EditorState, $end: ResolvedPos, dir: 1 | -1): number {
  let $at = $end;
  for (
    let pos = closedDiagramAt(state, $at);
    pos !== undefined;
    pos = closedDiagramAt(state, $at)
  ) {
    const node = state.doc.nodeAt(pos);
    if (!node) break;
    const beyond = state.doc.resolve(dir > 0 ? pos + node.nodeSize : pos);
    const found = Selection.findFrom(beyond, dir, true);
    if (!found) return beyond.pos;
    $at = found.$head;
  }
  return $at.pos;
}

/**
 * A selection with an end in the hidden source of a closed diagram (drawn
 * into it with Shift and an arrow, or left there when its place closed) takes
 * that diagram in whole, and never ends in text that cannot be seen, where
 * typing would replace part of it.
 */
function reachOver(state: EditorState): Transaction | null {
  const { $anchor, $head } = state.selection;
  const forward = $head.pos > $anchor.pos;
  const head = pastClosed(state, $head, forward ? 1 : -1);
  const anchor = pastClosed(state, $anchor, forward ? -1 : 1);
  if (head === $head.pos && anchor === $anchor.pos) return null;
  return state.tr.setSelection(TextSelection.create(state.doc, anchor, head));
}

/**
 * Whether the block at `pos` is the line the cursor was in, made a diagram by
 * typing its fence (three backticks and the language): not a diagram that was
 * already there, nor one that a new document or a paste brought.
 */
function madeHere(transactions: readonly Transaction[], before: EditorState, pos: number): boolean {
  const { $from } = before.selection;
  if ($from.depth === 0 || $from.parent.type.name !== 'paragraph') return false;
  if (!/^(?:```|~~~)/.test($from.parent.textContent)) return false;
  let at = $from.before();
  for (const tr of transactions) {
    const mapped = tr.mapping.mapResult(at, 1);
    if (mapped.deletedAcross) return false;
    at = mapped.pos;
  }
  return at === pos;
}

/** After a change from the room, the selection goes back to the open place it was in. */
export function keepPlaceSelected(
  transactions: readonly Transaction[],
  state: EditorState,
): Transaction | null {
  const place = placeOf(state);
  if (!place || !transactions.some(fromShared) || selectionStays(place, state)) return null;
  const node = state.doc.nodeAt(place.pos);
  if (!node) return null;
  if (place.kind === 'formula')
    return state.tr.setSelection(NodeSelection.create(state.doc, place.pos));
  const end = place.pos + node.nodeSize - 1;
  return state.tr.setSelection(TextSelection.create(state.doc, end));
}

/**
 * The cursor never rests in the source of a closed diagram: it selects the
 * diagram instead. A diagram that has just been made, empty, opens.
 */
export function keepOutOfClosed(
  transactions: readonly Transaction[],
  before: EditorState,
  state: EditorState,
): Transaction | null {
  const { selection } = state;
  if (!(selection instanceof TextSelection)) return null;
  const pos = closedDiagramAt(state, selection.$from);
  if (!selection.$to.sameParent(selection.$from)) return reachOver(state);
  if (pos === undefined) return null;
  const node = state.doc.nodeAt(pos);
  if (!node) return null;
  if (madeHere(transactions, before, pos) && node.content.size === 0)
    return state.tr.setMeta(placeKey, { open: { kind: 'diagram', pos } });
  return state.tr.setSelection(NodeSelection.create(state.doc, pos));
}

/** Whether the reader wrote something in a place's field. */
export function heldWritten(place: Place): boolean {
  if (place.kind === 'new-formula') return (place.panel?.value() ?? '') !== '';
  return place.draft?.touched === true;
}

/** Places closed on purpose (`closePlace`): what they held was written, or given up knowingly. */
const closings = new WeakSet<Place>();

/** Whether a place that is gone was closed on purpose, not taken away with its block. */
export function closedOnPurpose(place: Place): boolean {
  return closings.has(place);
}

/** Holds the open place's block again, once the reader's change is in the shared document. */
export function holdAgain(state: EditorState): void {
  const place = placeOf(state);
  if (!place?.anchor?.stale) return;
  place.anchor.held = holdNode(state, place.pos);
  place.anchor.stale = false;
}

/**
 * Opens the place of the block at `pos`: a diagram's with the cursor at the
 * end of its source, a formula's with the focus in its field.
 */
export function openPlace(view: EditorView, pos: number): boolean {
  const node = view.state.doc.nodeAt(pos);
  if (!view.editable || !node) return false;
  if (isDiagramNode(node)) {
    const tr = view.state.tr.setMeta(placeKey, { open: { kind: 'diagram', pos } });
    tr.setSelection(TextSelection.create(tr.doc, pos + node.nodeSize - 1));
    view.dispatch(tr.scrollIntoView());
    view.focus();
    return true;
  }
  if (node.type.name !== 'blockMath') return false;
  const latex = String(node.attrs.latex);
  // Until the formula's field says otherwise, what it holds is the stored source.
  const place: Place = { kind: 'formula', pos, draft: { value: () => latex } };
  const tr = view.state.tr.setMeta(placeKey, { open: place });
  view.dispatch(tr.setSelection(NodeSelection.create(tr.doc, pos)).scrollIntoView());
  return true;
}

/**
 * Closes the place that is open, on purpose: a formula keeps what its field
 * holds. With `focusBlock` (a key, its button) the block is selected and the
 * editor takes the focus; without (a press or the focus elsewhere), the
 * selection and the focus are left to what was pressed. With `dropping`,
 * nothing is written: what the field holds is given up (the reader may no
 * longer write). A place that only goes away (its block is deleted, the
 * document replaced) writes nothing either.
 */
export function closePlace(view: EditorView, focusBlock: boolean, dropping = false): void {
  const place = placeOf(view.state);
  if (!place) return;
  const tr = view.state.tr.setMeta(placeKey, { close: true });
  if (dropping) {
    view.dispatch(tr);
    return;
  }
  const at = place.kind === 'diagram' ? place.pos : finishFormula(tr, place);
  if (focusBlock) {
    if (at !== undefined) tr.setSelection(NodeSelection.create(tr.doc, at));
    else if (place.kind === 'formula')
      tr.setSelection(Selection.near(tr.doc.resolve(Math.min(place.pos, tr.doc.content.size))));
  }
  view.dispatch(tr);
  if (focusBlock) view.focus();
}

/** Opens the place of a new formula on the empty line at `pos`, as part of `tr`. */
export function openNewFormula(tr: Transaction, pos: number, panel: NewFormulaPanel): void {
  tr.setMeta(placeKey, { open: { kind: 'new-formula', pos, panel } });
}

/** Whether a key closes the place: Escape, or the platform's primary key with Enter (⌘Enter on Apple devices, Ctrl+Enter elsewhere). Never one an input method is composing with. */
export const closesPlace = (event: KeyboardEvent): boolean =>
  !isComposing(event) &&
  (event.key === 'Escape' || (event.key === 'Enter' && primaryModifier(event)));

/** The open place a node view's decorations carry, if they carry one. */
export function placeIn(decorations: readonly Decoration[]): Place | undefined {
  for (const decoration of decorations) {
    const spec = decoration.spec as { place?: Place };
    if (spec.place) return spec.place;
  }
  return undefined;
}
