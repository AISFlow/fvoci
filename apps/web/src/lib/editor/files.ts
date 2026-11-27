/**
 * From the identifier of a file to its address, what is known of it, and its
 * bytes.
 *
 * A document names an image or a file block by the identifier of an uploaded
 * file and never by an address. This is the one place that turns one into
 * the other; the app sets the source once it knows the board or wiki and the
 * base path, and the editor's nodes and the read-only view ask here.
 *
 * What the address assumes about the server (`getFile` in the contract;
 * the app sets the source, src/app/App.svelte and SharedApp.svelte):
 *
 * - The session is a cookie that scripts cannot read. The app never holds a
 *   token, so an address carries none: `<img src>` names the file directly
 *   and the browser sends the cookie. A board read by its public link is the
 *   one case where the address carries something: its read token, in the
 *   query of `getSharedFile` (`/shared/{containerId}/files/{fileId}`), which the
 *   reader's screen sets (src/app/SharedApp.svelte). (Saving a file under its
 *   own name reads its bytes by script, lib/save.ts; the server offers a
 *   download under that name too.)
 * - `GET` needs no extra request header, which is what lets a tag fetch a file.
 * - The API lives under `/api/v1`, below the base path the server is given.
 * - A file goes up in parts through an upload session (`api/upload.ts`). It
 *   has an identifier, and so an address, only once the upload is complete;
 *   until then the editor shows a placeholder and not a node.
 */

/** What a file block shows of its file: its record's name and size (`getFileRecord`). */
export interface FileDetails {
  name: string;
  /** In bytes. */
  size: number;
}

export type FileUrlResolver = (fileId: string) => string | undefined;

/**
 * How a file block opens its file in the viewer, where the app has one: the
 * editor names no viewer, and the app hands it this through the source.
 */
export interface FileViewerLink {
  /** Whether the viewer is likely to show the file, by its name and size. */
  opens(details: FileDetails): boolean;
  /**
   * Opens the viewer on the file; says whether the viewer came (it may not,
   * with the connection gone). `back` finds where the focus goes back to when
   * it closes: the name pressed (or the one drawn in its place meanwhile), or
   * the editor whose selection the block is.
   */
  open(fileId: string, details: FileDetails, back: () => HTMLElement | null): Promise<boolean>;
}

/** Where the files of the body shown come from. */
export interface FileSource {
  /** The address of a file, or undefined when it cannot be shown. */
  url: FileUrlResolver;
  /**
   * What is known of a file: `null` when there is none (gone, or not of the
   * board or wiki shown). Rejects when it cannot be asked now.
   */
  details?: (fileId: string) => Promise<FileDetails | null>;
  /**
   * The bytes of a file, to save it under its own name or to view it.
   * Rejects when they cannot be read, or are no longer wanted (`signal`).
   */
  bytes?: (fileId: string, signal?: AbortSignal) => Promise<Blob>;
  /** How a file block opens its file in the viewer; without it, a file is only downloaded. */
  view?: FileViewerLink;
  /**
   * Whose files the identifiers name now (the board or wiki shown), which
   * what is known of a file is kept under; `undefined` while none is, when
   * nothing is asked. Without it, everything is kept under one.
   */
  scope?: () => string | undefined;
}

let source: FileSource = { url: () => undefined };
/** What was asked of each file of each scope under the source of now; a failure is asked again. */
let asked = new Map<string, Promise<FileDetails | null>>();

/** Under what a file's details are kept: its identifier in its scope; none while there is none. */
function keyOf(fileId: string): string | undefined {
  if (!source.scope) return `/${fileId}`;
  const scope = source.scope();
  return scope === undefined ? undefined : `${scope}/${fileId}`;
}

/** Sets where files come from. Returns the previous source. */
export function setFileSource(next: FileSource): FileSource {
  const previous = source;
  source = next;
  asked = new Map();
  return previous;
}

/** The address of a file, or undefined when it cannot be shown. */
export function fileUrl(fileId: string): string | undefined {
  return source.url(fileId);
}

/**
 * What is known of a file: `null` when there is none, `undefined` when it
 * cannot be told (no source says, or the asking failed).
 */
export async function fileDetails(fileId: string): Promise<FileDetails | null | undefined> {
  const { details } = source;
  const key = keyOf(fileId);
  if (!details || key === undefined) return undefined;
  const mine = asked;
  let asking = mine.get(key);
  if (!asking) {
    asking = details(fileId);
    mine.set(key, asking);
  }
  try {
    return await asking;
  } catch {
    if (mine.get(key) === asking) mine.delete(key);
    return undefined;
  }
}

/** Forgets what is known of a file, which is asked again the next time (its download failed). */
export function forgetFile(fileId: string): void {
  const key = keyOf(fileId);
  if (key !== undefined) asked.delete(key);
}

/** The bytes of a file, or undefined when the source cannot give them. */
export function fileBytes(fileId: string): Promise<Blob> | undefined {
  return source.bytes?.(fileId);
}

/** How file blocks open the viewer, or undefined when the source names none. */
export function fileViewer(): FileViewerLink | undefined {
  return source.view;
}
