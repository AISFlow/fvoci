/** Helpers that work on plain text. */

/** Adds `https://` to an address that has no scheme. */
export function ensureProtocol(url: string): string {
  return /^.+:\/\//.test(url) ? url : `https://${url}`;
}

const ILLEGAL_IN_FILE_NAMES = /[\\/?:<>*|".]/g;

/** A name for a downloaded file, made from a title: without the characters file systems refuse. */
export function sanitizeFilename(filename: string): string {
  return filename.replace(ILLEGAL_IN_FILE_NAMES, '');
}

/**
 * A text in the form used for comparing without regard to case. It does not
 * depend on the language of the reader or the device: the same two texts
 * match for everyone. Compatibility forms are unified as well, so a
 * full-width `Ａ` matches `a` and Hangul typed as separate jamo matches the
 * composed syllable.
 */
export function foldCase(text: string): string {
  return text.normalize('NFKC').toLowerCase();
}

export function htmlEncode(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function htmlDecode(text: string): string {
  return text
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"');
}

/** A 32-bit hash of a text. For picking a colour or an icon, not for security. */
export function hashCode(text: string): number {
  let hash = 0;
  for (let index = 0; index < text.length; index += 1) {
    hash = (Math.imul(31, hash) + text.charCodeAt(index)) | 0;
  }
  return hash;
}

const FENCE = /^\s{0,3}(```|~~~)/;
const TASK_ITEM = /^\s*(?:[-+*]|\d{1,9}[.)])\s+\[([ xX])\]\s/;

/**
 * Counts the task list items of a Markdown text (`- [ ] open`, `- [x] done`).
 * Lines inside a fenced code block do not count.
 */
export function countCheckboxesInMarkdown(text: string): { total: number; checked: number } {
  let total = 0;
  let checked = 0;
  let inFence = false;
  for (const line of text.split('\n')) {
    if (FENCE.test(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const item = TASK_ITEM.exec(line);
    if (!item) continue;
    total += 1;
    if (item[1] !== ' ') checked += 1;
  }
  return { total, checked };
}
