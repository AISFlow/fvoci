/**
 * What the body of a card says about its room: the line under it while it
 * cannot be written (and why), and the notice when changes of the reader
 * were dropped with the document they were in. A wiki's page has a body of
 * the same kind, and says the same of it in its own words (`PAGE_WORDS`).
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { BodyDocState } from '$lib/state/backend.ts';

import type { KeepOutcome } from './body-versions.svelte.ts';

/** What a body is called in what it says: a card's description, or a wiki's page. */
export interface BodyWords {
  label: () => string;
  /** Said by an empty body to whoever may write in it. */
  placeholder: () => string;
  /** Said by an empty body to whoever may only read it. */
  empty: () => string;
  /** Said in the editor too while it may only be read; none for a card (its editor is left blank). */
  readEmpty?: () => string;
  invalid: () => string;
  joining: () => string;
  receiving: () => string;
  offline: () => string;
  readOnly: () => string;
  gone: () => string;
  unavailable: () => string;
  notOpened: () => string;
  lostReset: () => string;
  lostImage: () => string;
  lostFile: () => string;
  lostFormula: () => string;
}

/**
 * What is said of an ask to keep a version of the body now (`keepVersion`),
 * of a card's and of a page's alike.
 */
export const KEPT_SAID: Record<KeepOutcome, () => string> = {
  kept: m.card_version_kept,
  unchanged: m.card_version_unchanged,
  recent: m.card_version_recent,
  failed: m.card_version_keep_failed,
  notNow: m.card_version_keep_not_now,
  opening: m.card_version_keep_opening,
};

export const CARD_WORDS: BodyWords = {
  label: m.card_body,
  placeholder: m.card_body_placeholder,
  empty: m.card_body_empty,
  invalid: m.card_body_invalid,
  joining: m.card_body_joining,
  receiving: m.card_body_receiving,
  offline: m.card_body_offline,
  readOnly: m.card_body_read_only,
  gone: m.card_body_gone,
  unavailable: m.card_body_unavailable,
  notOpened: m.card_body_not_opened,
  lostReset: m.card_body_lost_reset,
  lostImage: m.card_body_lost_image,
  lostFile: m.card_body_lost_file,
  lostFormula: m.card_body_lost_formula,
};

export const PAGE_WORDS: BodyWords = {
  label: m.wiki_page_body,
  placeholder: m.wiki_page_body_placeholder,
  empty: m.wiki_page_body_empty,
  readEmpty: m.wiki_page_body_empty,
  invalid: m.wiki_body_invalid,
  joining: m.wiki_body_joining,
  receiving: m.wiki_body_receiving,
  offline: m.wiki_body_offline,
  readOnly: m.wiki_body_read_only,
  gone: m.wiki_body_gone,
  unavailable: m.wiki_body_unavailable,
  notOpened: m.wiki_body_not_opened,
  lostReset: m.wiki_body_lost_reset,
  lostImage: m.wiki_body_lost_image,
  lostFile: m.wiki_body_lost_file,
  lostFormula: m.wiki_body_lost_formula,
};

export const ENTRY_WORDS: BodyWords = {
  label: m.diary_entry_body,
  placeholder: m.diary_entry_placeholder,
  empty: m.diary_entry_empty,
  invalid: m.diary_body_invalid,
  joining: m.diary_body_joining,
  receiving: m.diary_body_receiving,
  offline: m.diary_body_offline,
  readOnly: m.diary_body_read_only,
  gone: m.diary_body_gone,
  unavailable: m.diary_body_unavailable,
  notOpened: m.diary_body_not_opened,
  lostReset: m.diary_body_lost_reset,
  lostImage: m.diary_body_lost_image,
  lostFile: m.diary_body_lost_file,
  lostFormula: m.diary_body_lost_formula,
};

export const UNIT_WORDS: BodyWords = {
  label: m.course_unit_body,
  placeholder: m.course_unit_body_placeholder,
  empty: m.course_unit_body_empty,
  readEmpty: m.course_unit_body_empty,
  invalid: m.course_body_invalid,
  joining: m.course_body_joining,
  receiving: m.course_body_receiving,
  offline: m.course_body_offline,
  readOnly: m.course_body_read_only,
  gone: m.course_body_gone,
  unavailable: m.course_body_unavailable,
  notOpened: m.course_body_not_opened,
  lostReset: m.course_body_lost_reset,
  lostImage: m.course_body_lost_image,
  lostFile: m.course_body_lost_file,
  lostFormula: m.course_body_lost_formula,
};

/** How long joining or receiving lasts before it is said, in milliseconds. */
export const LATE_NOTE = 1000;

export interface RoomNote {
  text: string;
  /** The reader can ask to be let in again now. */
  retry: boolean;
  /** Said only when it lasts: joining and receiving are usually over at once. */
  late: boolean;
}

/** The line for a room; none when the reader may write. */
export function roomNote(
  room: BodyDocState,
  canEdit: boolean,
  words: BodyWords = CARD_WORDS,
): RoomNote | undefined {
  const note = (text: string, more: Partial<RoomNote> = {}): RoomNote => ({
    text,
    retry: false,
    late: false,
    ...more,
  });
  switch (room.status) {
    case 'joining':
      return note(words.joining(), { late: true });
    case 'receiving':
      return note(words.receiving(), { late: true });
    case 'offline':
      return note(words.offline());
    case 'out':
      if (room.reason === 'not_found') return note(words.gone());
      // Not tried again by itself (the code of the room could not be fetched): the reader may.
      if (!room.retrying) return note(words.notOpened(), { retry: true });
      return note(words.unavailable(), { retry: true });
    case 'joined':
      if (room.mode === 'read' || !canEdit) return note(words.readOnly());
      // Waiting a moment for the server (a change it was out of step with): said when it lasts.
      if (!room.writable) return note(words.receiving(), { late: true });
      return undefined;
  }
}

/** Why an image that went up was not put in: the editor was made anew for another document. */
export const IMAGE_DROPPED = 'image_dropped';
/** Why a file that went up was not put in as a file block: the same. */
export const FILE_DROPPED = 'file_dropped';
/** Why a formula's source the reader wrote was not put in: the same. */
export const FORMULA_DROPPED = 'formula_dropped';

/** The notice for changes dropped, by the reason the room gave. */
export function lostNote(why: string, words: BodyWords = CARD_WORDS): string {
  if (why === 'document_reset') return words.lostReset();
  if (why === IMAGE_DROPPED) return words.lostImage();
  if (why === FILE_DROPPED) return words.lostFile();
  if (why === FORMULA_DROPPED) return words.lostFormula();
  if (why === 'table_too_large') return m.card_body_lost_table_large();
  if (why === 'update_too_large' || why === 'document_too_large') return m.card_body_lost_large();
  return m.card_body_lost();
}
