/**
 * What Markdown says of a toggle and a callout, as the server writes them
 * (crates/server/src/app/body_markdown/write.rs) and GitHub draws them:
 *
 * - a toggle is `<details>`, its summary's line in `<summary>…</summary>`,
 *   then the Markdown of what it hides, then `</details>`; one left open is
 *   closed at the end of the text;
 * - a callout is a GitHub alert: a quote whose first line is `[!NOTE]`,
 *   `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]` alone. An
 *   important one is a note: the tones have no other place for it.
 *
 * The reading of the rest is markdown.ts's; this finds where those start and
 * end, and what is inside.
 */
import type { Token, Tokens } from 'marked';

import type { CalloutTone } from './schema-rules.ts';

/** The tone of each kind of alert. */
const ALERTS: Record<string, CalloutTone> = {
  NOTE: 'info',
  TIP: 'tip',
  IMPORTANT: 'info',
  WARNING: 'warn',
  CAUTION: 'danger',
};

/** The first line of an alert, alone on it. */
const ALERT_LINE = /^[ \t]*\[!([A-Za-z]+)\][ \t]*(?:\r\n?|\n|$)/;

/** A quote that is an alert: its tone, and the Markdown of what is in it after the first line. */
export function alertOf(quote: Tokens.Blockquote): { tone: CalloutTone; body: string } | undefined {
  const line = ALERT_LINE.exec(quote.text);
  const tone = line?.[1] === undefined ? undefined : ALERTS[line[1].toUpperCase()];
  if (!line || tone === undefined) return undefined;
  return { tone, body: quote.text.slice(line[0].length) };
}

const OPEN = /<details(?:\s[^>]*)?>/gi;
const CLOSE = /<\/details\s*>/gi;
/**
 * A summary, from its tag to the end that no backslash of its own escapes:
 * what is written in it is its line's text, never a tag of a toggle.
 */
const SUMMARY = /<summary(?:\s[^>]*)?>[\s\S]*?(?<=(?:^|[^\\])(?:\\\\)*)<\/summary\s*>/gi;
const count = (text: string, pattern: RegExp): number => text.match(pattern)?.length ?? 0;
/** How deeper an HTML token takes the toggles it is in: its tags, not the text of a summary. */
const deepening = (raw: string): number => {
  const tags = raw.replace(SUMMARY, '');
  return count(tags, OPEN) - count(tags, CLOSE);
};

/** Whether a token of HTML opens a toggle. */
export const opensDetails = (token: Token): boolean =>
  token.type === 'html' && /^\s*<details(?:\s[^>]*)?>/i.test(token.raw);

/**
 * The tokens a toggle that opens at `start` is written in, to the one that
 * closes it (or to the end): how many there are, and their Markdown.
 */
export function detailsRun(
  tokens: readonly Token[],
  start: number,
): { length: number; raw: string } {
  let depth = 0;
  let raw = '';
  let index = start;
  for (; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (!token) break;
    raw += token.raw;
    if (token.type === 'html') depth += deepening(token.raw);
    if (depth <= 0) break;
  }
  return { length: Math.min(index, tokens.length - 1) - start + 1, raw };
}

/** A toggle's summary as written (Markdown of a line) and the Markdown of what it hides. */
export function detailsParts(raw: string): { summary: string; body: string } {
  let rest = raw.replace(/^\s*<details(?:\s[^>]*)?>/i, '');
  const end = rest.search(/<\/details\s*>\s*$/i);
  if (end >= 0) rest = rest.slice(0, end);
  // A `</summary>` after a backslash of its own is text of the summary.
  const summary =
    /^\s*<summary(?:\s[^>]*)?>([\s\S]*?)(?<=(?:^|[^\\])(?:\\\\)*)<\/summary\s*>/i.exec(rest);
  if (!summary) return { summary: '', body: rest };
  return { summary: (summary[1] ?? '').trim(), body: rest.slice(summary[0].length) };
}
