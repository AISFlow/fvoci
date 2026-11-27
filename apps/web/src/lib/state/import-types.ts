/**
 * Types for importing archives and external exports (`fvoci`, `trello`, `notion`)
 * through the backend boundary.
 */
import type { UploadOptions } from './backend.ts';

/** Source format accepted by `importArchive`. */
export type ImportFormat = 'fvoci' | 'trello' | 'notion';

/** Supported import formats in display order. */
export const IMPORT_FORMATS: readonly ImportFormat[] = ['fvoci', 'trello', 'notion'];

/** Counts of entities created by an import. */
export interface ImportCounts {
  readonly boards: number;
  readonly wikis: number;
  readonly courses?: number;
  readonly cards: number;
  readonly pages: number;
  readonly units?: number;
  readonly comments: number;
  readonly files: number;
}

/** Machine-readable category for a skipped or degraded import item. */
export type ImportWarningCode =
  | 'external_attachment_linked'
  | 'external_image_linked'
  | 'missing_local_asset'
  | 'closed_item_skipped'
  | 'content_truncated'
  | 'markdown_degraded'
  | 'unsupported_entry_skipped'
  | 'unresolved_reference'
  | 'csv_row_warning';

/** One skipped or degraded item recorded during an import. */
export interface ImportWarning {
  readonly code: ImportWarningCode;
  readonly item: string;
  readonly reason: string;
}

/** Result summary returned after a completed import. */
export interface ImportSummary {
  readonly format: ImportFormat;
  readonly imported: ImportCounts;
  /** How many members of a native archive's boards were left out because they are not in the workspace. */
  readonly membersLeftOut: number;
  /** Identifiers of the newly created containers, in creation order. */
  readonly containerIds: readonly string[];
  /** Items skipped during conversion or import. */
  readonly skipped: readonly ImportWarning[];
  /** Non-fatal degradation notices (e.g. external URLs preserved as links). */
  readonly warnings: readonly ImportWarning[];
  /** Additional notices omitted past the server reporting cap, if any. */
  readonly warningsOmitted?: number;
}

/** Alias kept for callers that refer to the completed import summary as `Imported`. */
export type Imported = ImportSummary;

/** Options for uploading and importing an archive or external export. */
export interface ImportOptions extends UploadOptions {
  /** Source format; defaults to `'fvoci'` when omitted. */
  format?: ImportFormat;
}

/** Empty `ImportCounts` baseline. */
export function emptyImportCounts(): ImportCounts {
  return { boards: 0, wikis: 0, courses: 0, cards: 0, pages: 0, units: 0, comments: 0, files: 0 };
}
