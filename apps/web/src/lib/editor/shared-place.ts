/**
 * Places in a body edited together, held across what others write.
 *
 * A position in the editor is a number: what others write before it moves
 * the place, and a change that comes from the shared document replaces the
 * blocks it changed in the editor's document, so the editor's own mapping
 * loses any position inside them. What is held for later is held here in
 * the shared document instead, and read back as a number when needed:
 *
 * - a node (a formula, a diagram, the empty line a new formula goes into)
 *   by its own shared element (`holdNode`): found wherever it is now, and
 *   gone when it was deleted, never taken for a neighbour;
 * - a spot in a line (where an image goes) by the letter before it, the
 *   end of its block and the block (`holdPlace`): followed through what is
 *   typed around it, after its line when the letter went into another
 *   block (an Enter writes the rest of a line anew), gone with its block.
 *
 * A hold is taken once the editor's own change is in the shared document
 * (`onShared`: after each of the binding's own writes into it). A plugin's
 * state may come before the binding's in a transaction: places are read
 * back with the binding of the state before it (`nodeNow`, `placeNow`
 * take that state and the new document). In an editor bound to no shared
 * document there is nothing to hold: the editor's mapping is all there is
 * (`undefined`).
 *
 * Also here: whether a read-only editor may still write what the reader
 * did before (`mayWrite`): an editor read-only for a moment (the
 * connection away) may, one whose reader may only read may not.
 */
import type { Node as DocumentNode } from '@tiptap/pm/model';
import type { EditorState, Transaction } from '@tiptap/pm/state';
import type { EditorView } from '@tiptap/pm/view';
import {
  absolutePositionToRelativePosition,
  relativePositionToAbsolutePosition,
  ySyncPluginKey,
} from '@tiptap/y-tiptap';
import * as Y from 'yjs';

import { hasInvalidContent } from './content-check.ts';

type Mapping = NonNullable<ReturnType<typeof ySyncPluginKey.getState>>['binding']['mapping'];
/** A shared element the editor draws a node from. */
type Shared = Mapping extends Map<infer Type, unknown> ? Type : never;

/** A node held by its shared element. */
export interface HeldNode {
  type: Shared;
}

/** A spot in a line held in the shared document. */
export interface HeldPlace {
  /** The spot: it names the letter before it (or the start of its block). */
  at: Y.RelativePosition;
  /** The spot after its block, when the spot is after a letter. */
  after?: Y.RelativePosition;
  /** The block it is in. */
  block?: Shared;
}

function bindingOf(state: EditorState) {
  const sync = ySyncPluginKey.getState(state);
  return sync?.binding ? { sync, mapping: sync.binding.mapping } : undefined;
}

/** The shared element an editor's node is drawn from. */
function sharedOf(mapping: Mapping, node: DocumentNode): Shared | undefined {
  for (const [type, drawn] of mapping) if (drawn === node) return type;
  return undefined;
}

/** Whether a shared type, or one it is in, was deleted. */
function deleted(type: Shared): boolean {
  let item = (type as Y.AbstractType<unknown>)._item;
  while (item) {
    if (item.deleted) return true;
    item = (item.parent as Y.AbstractType<unknown> | null)?._item ?? null;
  }
  return false;
}

/** Where a node of the editor's document is, if it is there. */
function positionOf(doc: DocumentNode, node: unknown): number | undefined {
  let found: number | undefined;
  doc.descendants((each, pos) => {
    if (found !== undefined) return false;
    if (each === node) found = pos;
    return found === undefined;
  });
  return found;
}

/** Holds the node at `pos` by its shared element; undefined in an editor bound to none. */
export function holdNode(state: EditorState, pos: number): HeldNode | undefined {
  const bound = bindingOf(state);
  const node = state.doc.nodeAt(pos);
  if (!bound || !node) return undefined;
  const type = sharedOf(bound.mapping, node);
  return type ? { type } : undefined;
}

/**
 * Where a held node is now in `doc`; `null` when it was deleted (by
 * anybody). `state` gives the binding: the state the change applies to.
 */
export function nodeNow(
  state: EditorState,
  held: HeldNode,
  doc: DocumentNode = state.doc,
): number | null | undefined {
  const bound = bindingOf(state);
  if (!bound) return undefined;
  if (deleted(held.type)) return null;
  return positionOf(doc, bound.mapping.get(held.type)) ?? null;
}

/** Holds the spot at `pos` in a line; undefined in an editor bound to none. */
export function holdPlace(state: EditorState, pos: number): HeldPlace | undefined {
  const bound = bindingOf(state);
  if (!bound) return undefined;
  const { sync, mapping } = bound;
  try {
    const at = absolutePositionToRelativePosition(pos, sync.type, mapping);
    const $pos = state.doc.resolve(pos);
    if (!$pos.parent.isTextblock) return { at };
    const block = sharedOf(mapping, $pos.parent);
    if ($pos.parentOffset === 0) return { at, block };
    const after = absolutePositionToRelativePosition($pos.after(), sync.type, mapping);
    return { at, after, block };
  } catch {
    return undefined;
  }
}

/** Whether the item a relative position names was deleted. */
function itemDeleted(doc: Y.Doc, position: Y.RelativePosition): boolean {
  if (!position.item) return false;
  try {
    return Y.getItem(doc.store, position.item).deleted;
  } catch {
    return true;
  }
}

/**
 * Where a held spot is now: a position, or `null` when its block was
 * deleted (by anybody, or made anew: turned into a heading, wrapped).
 */
export function placeNow(
  state: EditorState,
  held: HeldPlace,
  doc: DocumentNode = state.doc,
): number | null | undefined {
  const bound = bindingOf(state);
  if (!bound) return undefined;
  const { sync, mapping } = bound;
  if (held.block && deleted(held.block)) return null;
  const read = (position: Y.RelativePosition): number | null =>
    relativePositionToAbsolutePosition(sync.doc, sync.type, position, mapping);
  const letterGone = itemDeleted(sync.doc, held.at);
  const pos = letterGone && held.after ? read(held.after) : read(held.at);
  return pos === null ? null : Math.min(pos, doc.content.size);
}

/** Whether a transaction is a change of the shared document (another's, or the shared undo). */
export function fromShared(tr: Transaction): boolean {
  const meta = tr.getMeta(ySyncPluginKey) as { isChangeOrigin?: boolean } | undefined;
  return meta?.isChangeOrigin === true;
}

/**
 * Calls `then` after each write of the editor's own changes into the
 * shared document: when places moved by them may be held
 * again. Gives the way to stop; nothing in an editor bound to none.
 */
export function onShared(view: EditorView, then: () => void): () => void {
  const bound = bindingOf(view.state);
  if (!bound) return () => undefined;
  const doc = bound.sync.doc;
  const written = (transaction: Y.Transaction): void => {
    if (transaction.origin === ySyncPluginKey && !view.isDestroyed) then();
  };
  doc.on('afterTransaction', written);
  return () => {
    doc.off('afterTransaction', written);
  };
}

/** Why a formula's source the reader wrote was not kept: they may not write now, or its block is gone. */
export type DroppedWhy = 'refused' | 'gone';

interface Writer {
  /** The reader may still write while the editor is read-only for now. */
  may: () => boolean;
  /** Something the reader wrote (a formula's source) was not kept: to be said, with why. */
  dropped: (why: DroppedWhy) => void;
}

const writers = new WeakMap<EditorView, Writer>();

/** Says whether the reader of an editor may still write, and how a dropped source is said. */
export function setMayWrite(view: EditorView, writer: Writer): void {
  writers.set(view, writer);
}

/**
 * Whether what the reader did before may be written into the editor now: it
 * is editable, or read-only only for a moment (the room sends it later); and
 * the editor takes changes (not while it holds what it cannot draw).
 */
export function mayWrite(view: EditorView): boolean {
  if (hasInvalidContent(view.state)) return false;
  return view.editable || (writers.get(view)?.may() ?? false);
}

/** Says that what the reader wrote in a formula was not kept. */
export function sayDropped(view: EditorView, why: DroppedWhy): void {
  writers.get(view)?.dropped(why);
}
