/**
 * What the editor of a body (BodyEditor.svelte) takes and gives. Kept in a
 * file of its own so that a caller can name these types without importing
 * the component, which it fetches only when a body is first edited.
 */
import type { Editor } from '@tiptap/core';
import type { Snippet } from 'svelte';
import type { Awareness } from 'y-protocols/awareness';
import type { Doc } from 'yjs';

import type { SuggestionEntry } from './extensions.ts';
import type { ReferenceDrawing } from './reference-chip.ts';
import type { DocNode } from './schema.ts';
import type { UploadKind } from './uploads.ts';

/**
 * The shared type of a body's Y document that the editor binds to, and the
 * server makes the document in (`contracts/ydoc-vectors.json`).
 */
export const BODY_FIELD = 'default';

/** What a toolbar gets: the editor to run commands on, and a count that changes with its state. */
export interface BodyEditorTools {
  editor: Editor;
  /** Goes up with every change of the document or the selection; read it to stay current. */
  changes: number;
}

/**
 * What an editor draws its references from and puts them with: beyond the
 * drawing of a body that is only shown, what `[[` offers and what a pasted
 * address names.
 */
export interface BodyReferences extends ReferenceDrawing {
  /** What may be named whose title matches what was typed after `[[`. */
  search: (query: string) => SuggestionEntry[] | Promise<SuggestionEntry[]>;
  /** The cards of the same board that what was typed after `#` names; none without a board. */
  numbered?: (query: string) => SuggestionEntry[];
  /** The identifier an address names, if it is the address of a card or a page of the workspace. */
  named: (address: string) => string | undefined;
  /** Whether the reader may see what the identifier names. */
  seen: (id: string) => Promise<boolean>;
}

export interface BodyEditorProps {
  /** The stored document, `null` for an empty body. Read on creation and when `revision` changes. */
  doc: DocNode | null;
  /** Raise it to have `doc` read again after a change from outside. Unless it is raised, what is being written is never overwritten. */
  revision?: number;
  /** True unless given. */
  editable?: boolean;
  /**
   * The people who can be mentioned: by the name they are shown by, or by one
   * of the other names they are found by (`also`: the login name and the
   * parts of their name).
   */
  users?: readonly { id: string; name: string; also?: readonly string[] }[];
  /** The name of a user a document mentions; undefined when it is not known. */
  resolveUser?: (userId: string) => string | undefined;
  /** Every change, as the document that would be stored; `null` for an empty body. Debouncing is the caller's business. */
  onChange?: (doc: DocNode | null) => void;
  onFocus?: () => void;
  onBlur?: () => void;
  /**
   * What references are drawn from and put with. Without it every reference
   * shows as what the reader may not see, and none can be put.
   */
  references?: BodyReferences;
  /**
   * Uploads a file for the body (pasted, dropped or chosen through the slash
   * menu; a picture becomes an image, any other file a file block) and gives
   * its identifier. Without it no file can be added.
   */
  uploadFile?: (file: File) => Promise<string>;
  /** What an empty body says while it can be written. */
  placeholder?: string;
  /** What an empty body says while it can only be read; nothing unless given. */
  readPlaceholder?: string;
  /** Classes for the text field itself, next to `doc-body`. */
  class?: string;

  // Beyond the agreed shape, all optional:

  /** The accessible name of the text field; "Body" in the app's language unless given. */
  label?: string;
  /** Drawn above the text, inside the editor's frame: the caller's formatting buttons. */
  toolbar?: Snippet<[BodyEditorTools]>;
  /**
   * Called when `doc` has been read, on creation and after every `revision`:
   * with what the editor would store for it, or with `undefined` when the
   * document was refused and the editor is read-only. For a valid document
   * that is the document itself, but for what the editor settles its own
   * way (the marks of a text come in the editor's order).
   */
  onLoad?: (stored: DocNode | null | undefined) => void;
  /**
   * A body edited together: the editor is bound to this shared document
   * (its type `default`) instead of `doc`, which is then not read, nor is
   * `revision`. The undo of the editor takes back the reader's own changes
   * only. Another document needs another editor.
   */
  collab?: Doc;
  /**
   * With `collab`: the carets of that document (carets.ts). The others' are
   * drawn with their name (`resolveUser`) and their colour (`colorOf`, a CSS
   * colour by the user's identifier); where the reader's own is is written
   * into `awareness`. Read when the editor is made, as `collab` is.
   */
  carets?: { awareness: Awareness; colorOf: (userId: string) => string };
  /**
   * With `collab`: the reader began (`true`) or ended composing a syllable
   * with an input method. A change from outside in the line being composed
   * would end the composition: whoever brings such changes holds them back
   * meanwhile.
   */
  onComposing?: (composing: boolean) => void;
  /**
   * With `collab`: takes what has the editor read the selection of the page
   * and gives what takes it back. Whoever brings the others' changes calls
   * it before each (moves-kept.ts).
   */
  readsFirst?: (read: () => void) => () => void;
  /** Called once the editor is made, with it: for whoever puts the caret back where it was. */
  onEditor?: (editor: Editor) => void;
  /**
   * With `collab`: the reader may write, though the editor is read-only for
   * a moment (the connection away). What they did before (an image that
   * went up, a formula's field) is still written then; the room sends it.
   */
  canWrite?: boolean;
  /** With `collab`: an image or a file that went up had no place left, the editor being made anew meanwhile. */
  onUploadDropped?: (kind: UploadKind) => void;
  /**
   * With `collab`: the shared document holds something the editor could not
   * draw no longer. The editor took nothing from it meanwhile and is to be
   * made anew.
   */
  onFitsAgain?: () => void;
  /** With `collab`: the editor went away with a formula's source written and not yet in the body. */
  onDraftDropped?: () => void;
  /**
   * ⌘S or Ctrl+S was pressed in the editor (`save-key.ts`): a version of the
   * body is to be kept now. The browser's dialog to save the page never
   * opens, also without this.
   */
  onSave?: (() => void) | undefined;
}
