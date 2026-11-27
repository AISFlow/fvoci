/**
 * Files that are pasted or dropped into the editor, or chosen through its
 * slash menu: a picture the browser draws becomes an image, any other file a
 * file block.
 *
 * A document names an image or a file block by the identifier of an uploaded
 * file, and a file has an identifier only once its upload is complete
 * (files.ts). So a file is handed to the caller's `upload`, a placeholder
 * marks where its node will go, and the node is inserted when the identifier
 * arrives. The placeholder is a decoration, no part of the document: nothing
 * about an upload in progress is ever stored. Without an `upload` function
 * the editor takes no files at all.
 *
 * In a body edited together the place is held in the shared document
 * (shared-place.ts), so that what others write meanwhile, before it or in
 * its block, does not move the node elsewhere. The node is not put in,
 * and that is said, when the block it was going into was deleted or made
 * anew (by anybody; by the reader, when the spot itself is taken away),
 * when the reader may no longer write, and when the editor takes no change
 * for now (content it cannot draw). An editor read only for a moment (the
 * connection away) takes it: the room sends it later.
 */
import { Extension } from '@tiptap/core';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { Plugin, PluginKey, type EditorState, type Transaction } from '@tiptap/pm/state';
import { insertPoint } from '@tiptap/pm/transform';
import { Decoration, DecorationSet, type EditorView } from '@tiptap/pm/view';

import {
  fromShared,
  holdPlace,
  mayWrite,
  onShared,
  placeNow,
  type HeldPlace,
} from './shared-place.ts';

/**
 * How an upload ended for the reader: put in, failed (or given up), its
 * place deleted, or the editor no longer taking it in.
 */
export type UploadOutcome = 'done' | 'failed' | 'gone' | 'refused';

/** The node an upload becomes: an image, or a file block. */
export type UploadKind = 'image' | 'file';

export interface BodyUploadOptions {
  /** Uploads a file and gives its identifier. Undefined: no file can be added. */
  upload: () => ((file: File) => Promise<string>) | undefined;
  /** What the placeholder of an upload of the kind says to a screen reader. */
  uploadingLabel: (kind: UploadKind) => string;
  /**
   * How an upload that just ended did, and of which kind. Also once the
   * editor is gone (`destroyed`): a node that could not be put in is said by
   * whoever is still there.
   */
  onProgress: (ended: { outcome: UploadOutcome; kind: UploadKind }, destroyed: boolean) => void;
}

export interface BodyUploadStorage {
  /**
   * Asks the reader for files and puts each in where the cursor is once they
   * are chosen. False when the editor takes no files now. Called from what
   * the reader did (a key, a press), which a browser asks before it opens
   * its dialog.
   */
  choose: () => boolean;
}

declare module '@tiptap/core' {
  interface Storage {
    bodyUpload: BodyUploadStorage;
  }
}

/**
 * What this extension does to its placeholders: adds one, or takes one away,
 * when its node is put in, moving those that waited at the same spot to just
 * after that node (`moved`).
 */
type Change =
  | { add: { id: number; position: number; label: string; kind: UploadKind } }
  | { remove: number; moved?: { ids: number[]; to: number } };

/** Where a node goes once its file is up. */
interface Placeholder {
  id: number;
  kind: UploadKind;
  pos: number;
  /** The place in the shared document, in a body edited together. */
  held: HeldPlace | undefined;
  /** Moved by the reader's own change: held again once it is in the shared document. */
  stale: boolean;
  label: string;
  /** Its block was deleted: the node has nowhere to go. */
  gone: boolean;
}

const key = new PluginKey<Placeholder[]>('bodyUpload');

/** Whether a transaction is this extension's own (a placeholder added or taken away, a node put in). */
export const fromUpload = (tr: Transaction): boolean => tr.getMeta(key) !== undefined;

/** Where a placeholder is after a transaction. */
function follow(placeholder: Placeholder, tr: Transaction, before: EditorState): Placeholder {
  if (placeholder.gone) return placeholder;
  if (placeholder.held && fromShared(tr)) {
    const now = placeNow(before, placeholder.held, tr.doc);
    if (now === null) return { ...placeholder, gone: true };
    if (now !== undefined) return { ...placeholder, pos: now };
  }
  if (!tr.docChanged) return placeholder;
  // The reader's own change took away the spot itself (its line deleted, or text around it
  // replaced): the node has nowhere to go, as when somebody else deleted it.
  const mapped = tr.mapping.mapResult(placeholder.pos);
  if (mapped.deletedAcross) return { ...placeholder, gone: true };
  return { ...placeholder, pos: mapped.pos, stale: true };
}

/** Where the node of a placeholder goes now: a position, `null` when its block is gone. */
function placeOf(state: EditorState, placeholder: Placeholder): number | null {
  if (placeholder.gone) return null;
  if (!placeholder.held) return placeholder.pos;
  return placeNow(state, placeholder.held) ?? null;
}

/** The placeholder in the page: a picture's box, or a line where a file block will be. */
function widget(placeholder: Placeholder, size: number): Decoration {
  const { id, label, kind } = placeholder;
  return Decoration.widget(
    Math.min(placeholder.pos, size),
    (view) => {
      const element = view.dom.ownerDocument.createElement('span');
      element.setAttribute('class', 'doc-upload');
      element.setAttribute('data-kind', kind);
      element.setAttribute('role', kind === 'image' ? 'img' : 'group');
      element.setAttribute('aria-label', label);
      return element;
    },
    { id, key: `doc-upload-${String(id)}` },
  );
}

/**
 * Puts `node` in at `at` as `replaceRangeWith` does, and gives where it ends:
 * where a node that was to go to the same spot goes after it. At the start or
 * the end of a line with text the node goes before or after that block; in
 * its middle it splits it; an empty line it takes the place of.
 */
function insertAt(transaction: Transaction, at: number, node: DocumentNode): number {
  const $at = transaction.doc.resolve(at);
  const point =
    !node.isInline && $at.parent.content.size > 0
      ? insertPoint(transaction.doc, at, node.type)
      : null;
  transaction.replaceRangeWith(at, at, node);
  return point === null ? transaction.mapping.map(at, 1) : point + node.nodeSize;
}

/** Puts the node of an upload that ended where its placeholder is now, if it may. */
function putIn(
  view: EditorView,
  id: number,
  kind: UploadKind,
  fileId: string | undefined,
): UploadOutcome {
  const placeholder = key.getState(view.state)?.find((each) => each.id === id);
  const at = placeholder ? placeOf(view.state, placeholder) : null;
  const remove = (): void => {
    view.dispatch(view.state.tr.setMeta(key, { remove: id } satisfies Change));
  };
  const type = view.state.schema.nodes[kind];
  if (fileId === undefined || fileId === '' || !type) {
    remove();
    return 'failed';
  }
  if (at === null || !mayWrite(view)) {
    remove();
    return at === null ? 'gone' : 'refused';
  }
  const before = view.state.doc;
  // The others that were to go to the same spot (files chosen together) go after this one.
  const waiting = (key.getState(view.state) ?? [])
    .filter((other) => other.id !== id && placeOf(view.state, other) === at)
    .map((other) => other.id);
  const transaction = view.state.tr;
  const after = insertAt(transaction, at, type.create({ fileId }));
  const moved = waiting.length > 0 ? { ids: waiting, to: after } : undefined;
  view.dispatch(transaction.setMeta(key, { remove: id, moved } satisfies Change));
  if (view.state.doc !== before) return 'done';
  // Not taken (the editor takes no change while it holds content it cannot draw).
  remove();
  return 'refused';
}

/**
 * The kinds of picture that become images. The server serves every file as
 * bytes of no kind (`application/octet-stream`, `nosniff`); a browser still
 * draws these by what they are, but not an SVG, nor kinds it does not draw
 * at all: those, and every other file, become file blocks.
 */
const IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif']);

const uploadKindOf = (file: File): UploadKind => (IMAGE_TYPES.has(file.type) ? 'image' : 'file');

const filesIn = (files: FileList | null | undefined): File[] => Array.from(files ?? []);

/** Marks the input of `askForFiles` in the page. */
const ASKING = 'data-body-files';

/** Opens the browser's dialog for files, and hands over what is chosen; nothing when none is. */
function askForFiles(page: Document, chosen: (files: File[]) => void): void {
  // One left by a dialog closed unheard (a browser that tells no `cancel`) goes now.
  for (const left of page.querySelectorAll(`input[${ASKING}]`)) left.remove();
  const input = page.createElement('input');
  input.setAttribute(ASKING, '');
  input.type = 'file';
  input.multiple = true;
  // In the page while the dialog is open (some browsers open none for a detached input), unseen.
  input.hidden = true;
  const done = (): void => {
    input.remove();
  };
  input.addEventListener('change', () => {
    const files = filesIn(input.files);
    done();
    if (files.length > 0) chosen(files);
  });
  input.addEventListener('cancel', done);
  page.body.append(input);
  input.click();
}

/**
 * What starts the uploads of files at a place in the editor: each file gets
 * its placeholder at once, and its node there once it is up. Files given
 * together go up together, and their nodes are put in in the order they
 * were given: one waits for those before it to end. Whether any was
 * started: none without an upload, files or an editable editor.
 */
function uploader(
  options: BodyUploadOptions,
): (view: EditorView, files: File[], position: number) => boolean {
  let serial = 0;

  return function start(view: EditorView, files: File[], position: number): boolean {
    const upload = options.upload();
    if (!upload || files.length === 0 || !view.editable) return false;
    let previous: Promise<unknown> = Promise.resolve();
    for (const file of files) {
      serial += 1;
      const id = serial;
      const kind = uploadKindOf(file);
      view.dispatch(
        view.state.tr.setMeta(key, {
          add: { id, position, label: options.uploadingLabel(kind), kind },
        } satisfies Change),
      );
      const finish = (fileId: string | undefined): void => {
        if (view.isDestroyed) {
          // The editor was made anew (another document of the room): the node has no place.
          const outcome = fileId === undefined ? 'failed' : 'gone';
          options.onProgress({ outcome, kind }, true);
          return;
        }
        options.onProgress({ outcome: putIn(view, id, kind, fileId), kind }, false);
      };
      const ended = upload(file).then(
        (fileId) => fileId,
        () => undefined,
      );
      previous = previous
        .then(
          () => ended,
          () => ended,
        )
        .then(finish);
    }
    return true;
  };
}

export const BodyUpload = Extension.create<BodyUploadOptions, BodyUploadStorage>({
  name: 'bodyUpload',

  addOptions() {
    return { upload: () => undefined, uploadingLabel: () => '', onProgress: () => undefined };
  },

  addStorage() {
    return { choose: () => false };
  },

  addProseMirrorPlugins() {
    const { options, editor, storage } = this;
    const start = uploader(options);

    storage.choose = () => {
      const view = editor.view;
      if (!options.upload() || !view.editable) return false;
      askForFiles(view.dom.ownerDocument, (files) => {
        // Where the cursor is once they are chosen: the dialog may have been open a while.
        if (!view.isDestroyed) start(view, files, view.state.selection.from);
      });
      return true;
    };

    return [
      new Plugin<Placeholder[]>({
        key,
        state: {
          init: () => [],
          apply(transaction, list, before) {
            let next = list.map((placeholder) => follow(placeholder, transaction, before));
            const change = transaction.getMeta(key) as Change | undefined;
            if (change && 'add' in change) {
              const { id, position, label, kind } = change.add;
              const pos = Math.min(position, transaction.doc.content.size);
              next = [...next, { id, kind, pos, held: undefined, stale: true, label, gone: false }];
            } else if (change && 'remove' in change) {
              const { remove, moved } = change;
              next = next
                .filter((placeholder) => placeholder.id !== remove)
                .map((placeholder) =>
                  moved?.ids.includes(placeholder.id)
                    ? { ...placeholder, pos: moved.to, held: undefined, stale: true, gone: false }
                    : placeholder,
                );
            }
            return next;
          },
        },
        // Held once the change that put it there is in the shared document (shared-place.ts):
        // after the binding writes it, or at once when nothing was to be written (the body empty).
        view: (view) => {
          const hold = (): void => {
            for (const placeholder of key.getState(view.state) ?? []) {
              if (!placeholder.stale || placeholder.gone) continue;
              placeholder.held = holdPlace(view.state, placeholder.pos);
              placeholder.stale = false;
            }
          };
          return {
            update: (_view, before) => {
              if (view.state.doc === before.doc) hold();
            },
            destroy: onShared(view, hold),
          };
        },
        props: {
          decorations: (state) => {
            const list = (key.getState(state) ?? []).filter((placeholder) => !placeholder.gone);
            if (list.length === 0) return DecorationSet.empty;
            const size = state.doc.content.size;
            return DecorationSet.create(
              state.doc,
              list.map((placeholder) => widget(placeholder, size)),
            );
          },
          handlePaste: (view, event) => {
            const files = filesIn(event.clipboardData?.files);
            if (!start(view, files, view.state.selection.from)) return false;
            event.preventDefault();
            return true;
          },
          handleDrop: (view, event) => {
            const files = filesIn(event.dataTransfer?.files);
            const at = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos;
            if (!start(view, files, at ?? view.state.selection.from)) return false;
            event.preventDefault();
            return true;
          },
        },
      }),
    ];
  },
});
