/**
 * The messages between the app and the frame the HWP editor runs in.
 *
 * The frame is as the viewer's is (protocol.ts): a document
 * without an origin of its own and without the network. The app hands it the
 * bytes of the version being edited; the frame answers that the document is
 * open, that it changed, that the writer asked to save, and, when the app asks
 * for them, the bytes to save together with what rhwp says those bytes lose
 * of the document. The app stores them through its own API; the frame stores
 * nothing and reaches nobody.
 *
 * The bytes never come without the report: one message carries both, and a
 * report the frame cannot read is an export that failed.
 *
 * Each side reads what arrives with the readers below, which accept exactly
 * these shapes and nothing more; the app also checks that a message came from
 * its own frame, and the frame that it came from its parent.
 */
import { PAGE_LIMIT, type Theme } from './protocol.ts';

/** The formats the editor edits, each saved as itself. */
export const EDIT_FORMATS = ['hwp', 'hwpx'] as const;
export type EditFormat = (typeof EDIT_FORMATS)[number];

/** Why the frame could not open the document for editing. */
export const EDIT_FAILURES = ['unreadable', 'locked', 'too-large', 'editor'] as const;
export type EditFailure = (typeof EDIT_FAILURES)[number];

/**
 * What a saved file loses: as rhwp's report words it (rhwp-studio,
 * core/export-content-loss.ts), a picture or other binary data written empty,
 * a control left out, the parameters of a field reduced; and, as the frame
 * finds it, a part of an HWPX package or a stream of an HWP that rhwp does
 * not keep (`partOmitted`, editor-frame/parts.ts and streams.ts), which its
 * report does not name: the preview Hangul makes again as it saves a file
 * among them (`previewMadeAgain`).
 */
export const LOSS_CODES = [
  'binaryContentEmptied',
  'controlOmitted',
  'metadataReduced',
  'partOmitted',
] as const;
export const LOSS_SUBJECTS = [
  'binaryData',
  'control',
  'fieldParameters',
  'packagePart',
  'stream',
] as const;
export const LOSS_REASONS = [
  'resourceReadFailedOrLimitExceeded',
  'storedCompressionMismatch',
  'rawPassthroughUnavailable',
  'unsupportedByOutputFormat',
  'notKeptByEditor',
  'previewMadeAgain',
] as const;

export interface Loss {
  code: (typeof LOSS_CODES)[number];
  subject: (typeof LOSS_SUBJECTS)[number];
  reason: (typeof LOSS_REASONS)[number];
  /** Where in the document, as rhwp names it. */
  path: string;
}

/** The most losses a message lists (`count` says how many there are), and the longest path. */
export const LOSS_LIST_LIMIT = 200;
export const LOSS_PATH_LIMIT = 300;
/** The most losses a report may count; more is a broken message. */
export const LOSS_COUNT_LIMIT = 10_000_000;
/** The longest name of a file, in UTF-16 units. */
export const NAME_LIMIT = 255;
/** The most requests to export a session numbers. */
export const REQUEST_LIMIT = 1_000_000;

/**
 * A count the saved file, read back as the viewer reads it, has otherwise than
 * the document in the editor: its pages, or its controls of one kind (Hangul's
 * 조판 부호, such as `tbl` a table or `pgnp` the place of the page numbers), by
 * the id the format gives the kind. The frame sends the ids; the app names them.
 */
export type Recount =
  | { of: 'pages'; before: number; after: number }
  | { of: 'controls'; id: string; before: number; after: number };
/**
 * A kind's id as the format gives it, as rhwp trims it: up to four printable
 * ASCII characters, none a space at either end (`tbl`, not `tbl `), or none.
 */
export const CONTROL_ID = /^(?:[\x21-\x7e](?:[\x20-\x7e]{0,2}[\x21-\x7e])?)?$/;
/** The most counts one save carries; past it, the first are told. */
export const RECOUNT_LIMIT = 64;
/** The most controls of a kind a count says. */
export const CONTROL_COUNT_LIMIT = 10_000_000;

export type ToEditor =
  | {
      kind: 'open';
      format: EditFormat;
      name: string;
      bytes: ArrayBuffer;
      /** rhwp's WebAssembly, which the frame cannot fetch (engine.ts). */
      engine: ArrayBuffer;
      theme: Theme;
      lang: string;
    }
  /** Asks for the document as it is now, to be saved; answered with `exported` or `export-failed`. */
  | { kind: 'export'; request: number }
  /**
   * What `request` exported is stored, as the file's new version or as a file of its own: the
   * document is unchanged since, unless it changed again.
   */
  | { kind: 'stored'; request: number; as: 'version' | 'apart' };

export type FromEditor =
  | { kind: 'ready' }
  | { kind: 'opened'; pages: number }
  | { kind: 'failed'; reason: EditFailure }
  /** The document has changes that are not stored, or has none any more. */
  | { kind: 'changed'; dirty: boolean }
  /** The writer asked, in the frame, to save (Ctrl+S). */
  | { kind: 'save-asked' }
  /** The writer asked, in the frame, to go to the app's controls (F6): the frame keeps the keys. */
  | { kind: 'leave' }
  /** The writer is at work in the document (a key, the pointer, a change): the lease is held. */
  | { kind: 'input' }
  | {
      kind: 'exported';
      request: number;
      bytes: ArrayBuffer;
      count: number;
      losses: Loss[];
      /** The document is a distribution document (배포용 문서), and the bytes are not one. */
      unprotected: boolean;
      /** What the saved file, read back, counts otherwise than the document in the editor. */
      recounts: Recount[];
    }
  | { kind: 'export-failed'; request: number };

type Fields = Record<string, unknown>;

/** The fields of `data` when it is a plain object with exactly these keys. */
function exactly(data: unknown, keys: readonly string[]): Fields | undefined {
  if (typeof data !== 'object' || data === null || Array.isArray(data)) return undefined;
  if (Object.getPrototypeOf(data) !== Object.prototype) return undefined;
  const own = Object.keys(data);
  if (own.length !== keys.length || !keys.every((key) => own.includes(key))) return undefined;
  return data as Fields;
}

const isOneOf = <T extends string>(list: readonly T[], value: unknown): value is T =>
  typeof value === 'string' && (list as readonly string[]).includes(value);

const isTheme = (value: unknown): value is Theme => value === 'light' || value === 'dark';

const isLanguage = (value: unknown): value is string =>
  typeof value === 'string' && /^[A-Za-z]{2,3}(?:-[A-Za-z0-9]{1,8}){0,3}$/.test(value);

const isCount = (value: unknown, most: number): value is number =>
  Number.isInteger(value) && (value as number) >= 0 && (value as number) <= most;

const isRequest = (value: unknown): value is number => isCount(value, REQUEST_LIMIT) && value >= 1;

const isName = (value: unknown): value is string =>
  typeof value === 'string' && value !== '' && value.length <= NAME_LIMIT;

function lossOf(data: unknown): Loss | undefined {
  const fields = exactly(data, ['code', 'subject', 'reason', 'path']);
  if (!fields) return undefined;
  const { code, subject, reason, path } = fields;
  if (!isOneOf(LOSS_CODES, code) || !isOneOf(LOSS_SUBJECTS, subject)) return undefined;
  if (!isOneOf(LOSS_REASONS, reason)) return undefined;
  if (typeof path !== 'string' || path.length > LOSS_PATH_LIMIT) return undefined;
  return { code, subject, reason, path };
}

function recountOf(data: unknown): Recount | undefined {
  const of = (data as { of?: unknown } | null)?.of;
  const fields = exactly(
    data,
    of === 'pages' ? ['of', 'before', 'after'] : ['of', 'id', 'before', 'after'],
  );
  if (!fields) return undefined;
  const { id, before, after } = fields;
  const most = of === 'pages' ? PAGE_LIMIT : CONTROL_COUNT_LIMIT;
  // A count that did not change is not one to tell.
  if (!isCount(before, most) || !isCount(after, most) || before === after) return undefined;
  // A document opens on a page at least.
  if (of === 'pages') return before >= 1 && after >= 1 ? { of, before, after } : undefined;
  if (of !== 'controls' || typeof id !== 'string' || !CONTROL_ID.test(id)) return undefined;
  return { of, id, before, after };
}

/** The counts as the frame tells them: each once, the pages first. */
function recountsOf(data: unknown): Recount[] | undefined {
  if (!Array.isArray(data) || data.length > RECOUNT_LIMIT) return undefined;
  const read = data.map(recountOf);
  if (!read.every((recount) => recount !== undefined)) return undefined;
  const keys = read.map((recount) => (recount.of === 'pages' ? undefined : recount.id));
  if (new Set(keys).size !== keys.length || keys.indexOf(undefined) > 0) return undefined;
  return read;
}

function exportedOf(data: unknown): Extract<FromEditor, { kind: 'exported' }> | undefined {
  const keys = ['kind', 'request', 'bytes', 'count', 'losses', 'unprotected', 'recounts'];
  const fields = exactly(data, keys);
  if (!fields) return undefined;
  const { request, bytes, count, losses, unprotected } = fields;
  if (!isRequest(request) || !(bytes instanceof ArrayBuffer)) return undefined;
  if (typeof unprotected !== 'boolean') return undefined;
  const recounts = recountsOf(fields.recounts);
  if (!recounts) return undefined;
  if (!isCount(count, LOSS_COUNT_LIMIT) || !Array.isArray(losses)) return undefined;
  // A count that is not the list's, where the list is whole, is a report that does not agree
  // with itself; a longer one is cut at the limit, and only there.
  if (losses.length !== Math.min(count, LOSS_LIST_LIMIT)) return undefined;
  const read = losses.map(lossOf);
  if (!read.every((loss) => loss !== undefined)) return undefined;
  return { kind: 'exported', request, bytes, count, losses: read, unprotected, recounts };
}

function kindOf(data: unknown): unknown {
  return typeof data === 'object' && data !== null ? (data as Fields).kind : undefined;
}

/** A message from the frame, if `data` is exactly one. */
export function readFromEditor(data: unknown): FromEditor | undefined {
  switch (kindOf(data)) {
    case 'ready':
      return exactly(data, ['kind']) ? { kind: 'ready' } : undefined;
    case 'opened': {
      const pages = exactly(data, ['kind', 'pages'])?.pages;
      return isCount(pages, PAGE_LIMIT) && pages >= 1 ? { kind: 'opened', pages } : undefined;
    }
    case 'failed': {
      const reason = exactly(data, ['kind', 'reason'])?.reason;
      return isOneOf(EDIT_FAILURES, reason) ? { kind: 'failed', reason } : undefined;
    }
    case 'changed': {
      const dirty = exactly(data, ['kind', 'dirty'])?.dirty;
      return typeof dirty === 'boolean' ? { kind: 'changed', dirty } : undefined;
    }
    case 'save-asked':
    case 'leave':
    case 'input': {
      const kind = kindOf(data) as 'save-asked' | 'leave' | 'input';
      return exactly(data, ['kind']) ? { kind } : undefined;
    }
    case 'exported':
      return exportedOf(data);
    case 'export-failed': {
      const request = exactly(data, ['kind', 'request'])?.request;
      return isRequest(request) ? { kind: 'export-failed', request } : undefined;
    }
    default:
      return undefined;
  }
}

/** A message from the app, if `data` is exactly one. */
export function readToEditor(data: unknown): ToEditor | undefined {
  switch (kindOf(data)) {
    case 'open': {
      const keys = ['kind', 'format', 'name', 'bytes', 'engine', 'theme', 'lang'];
      const fields = exactly(data, keys);
      if (!fields || !isOneOf(EDIT_FORMATS, fields.format) || !isName(fields.name))
        return undefined;
      const { format, name, bytes, engine, theme, lang } = fields;
      if (!(bytes instanceof ArrayBuffer) || !(engine instanceof ArrayBuffer)) return undefined;
      if (!isTheme(theme) || !isLanguage(lang)) return undefined;
      return { kind: 'open', format, name, bytes, engine, theme, lang };
    }
    case 'export': {
      const request = exactly(data, ['kind', 'request'])?.request;
      return isRequest(request) ? { kind: 'export', request } : undefined;
    }
    case 'stored': {
      const fields = exactly(data, ['kind', 'request', 'as']);
      if (!fields || !isRequest(fields.request)) return undefined;
      const { request, as } = fields;
      return as === 'version' || as === 'apart' ? { kind: 'stored', request, as } : undefined;
    }
    default:
      return undefined;
  }
}
