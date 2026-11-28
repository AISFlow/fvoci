/**
 * An iCalendar file (RFC 5545) of dated things: one `VEVENT` each, with a
 * `UID` of its own that stays the same from one file to the next (so that a
 * calendar the file is opened in again updates its events in place of adding
 * them twice), the `DTSTAMP` of the file, and its days or moments.
 *
 * A day is a `VALUE=DATE`, and the end of days is the day after the last of
 * them, as RFC 5545 has it (`DTEND` is not part of the event): a card of the
 * 3rd to the 5th ends on the 6th. A moment is written in UTC (`Z`). A range
 * that ends before it starts is read the other way round, and one of a single
 * moment has no end. A text is escaped (`\\`, `;`, `,`, a new line) and holds
 * no other control character, which RFC 5545 does not take in a text; a link
 * is one of the web (`http:`, `https:`), or there is none. A line longer than
 * 75 octets is folded, never inside a character of UTF-8, and no line holds a
 * break of its own.
 */
import { addDays, type CalendarDate, type DateValue } from './dates.ts';

/** One event of the file. */
export interface IcsEvent {
  /** Stays the same for the same thing: a card's identifier, say. */
  readonly uid: string;
  readonly title: string;
  readonly when: DateValue;
  /** A link to the thing, if it has one. */
  readonly url?: string | undefined;
}

const encoder = new TextEncoder();

/**
 * The control characters a text of iCalendar does not hold: all but a tab and
 * the line breaks it escapes.
 */
// eslint-disable-next-line no-control-regex -- these are what it finds
const CONTROLS = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g;

/** A text as a value of iCalendar has it. */
export function icsText(text: string): string {
  return text
    .replace(CONTROLS, '')
    .replace(/\\/g, '\\\\')
    .replace(/;/g, '\\;')
    .replace(/,/g, '\\,')
    .replace(/\r\n|\r|\n/g, '\\n');
}

/**
 * The address of a link of an event, if it is one of the web; none for another
 * (`javascript:`, say) or for one that would break its line.
 */
export function icsUrl(url: string | undefined): string | undefined {
  // eslint-disable-next-line no-control-regex -- these are what it refuses
  if (!url || /[\u0000-\u001F\u007F]/.test(url)) return undefined;
  try {
    const parsed = new URL(url);
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.href : undefined;
  } catch {
    return undefined;
  }
}

/** A line folded at 75 octets, a fold never inside a character; a break it holds goes. */
export function foldLine(line: string): string {
  const parts: string[] = [];
  let part = '';
  let octets = 0;
  for (const character of line.replace(/[\r\n]/g, '')) {
    const size = encoder.encode(character).length;
    // The first line holds 75 octets, each after it 74 after the space it starts with.
    const room = parts.length === 0 ? 75 : 74;
    if (octets + size > room) {
      parts.push(part);
      part = '';
      octets = 0;
    }
    part += character;
    octets += size;
  }
  parts.push(part);
  return parts.join('\r\n ');
}

const pad = (value: number, width = 2): string => String(value).padStart(width, '0');

function dayOf(date: CalendarDate): string {
  return date.replace(/-/g, '');
}

function momentOf(instant: number): string {
  const at = new Date(instant);
  return (
    `${pad(at.getUTCFullYear(), 4)}${pad(at.getUTCMonth() + 1)}${pad(at.getUTCDate())}` +
    `T${pad(at.getUTCHours())}${pad(at.getUTCMinutes())}${pad(at.getUTCSeconds())}Z`
  );
}

/**
 * The lines of when an event is, or none for a value without a start. A range
 * whose end comes before its start is read from the earlier to the later.
 */
function whenLines(when: DateValue): string[] | undefined {
  const { from, to } = when;
  if (!from) return undefined;
  if (from.date !== undefined) {
    const other = to?.date ?? from.date;
    const [first, last] = other < from.date ? [other, from.date] : [from.date, other];
    return [`DTSTART;VALUE=DATE:${dayOf(first)}`, `DTEND;VALUE=DATE:${dayOf(addDays(last, 1))}`];
  }
  const other = to?.instant ?? from.instant;
  const [start, end] = [Math.min(from.instant, other), Math.max(from.instant, other)];
  const begins = `DTSTART:${momentOf(start)}`;
  return end === start ? [begins] : [begins, `DTEND:${momentOf(end)}`];
}

/** The file of `events` made at `now`, named `name` in a calendar it is added to. */
export function icsCalendar(name: string, events: readonly IcsEvent[], now: number): string {
  const lines = [
    'BEGIN:VCALENDAR',
    'VERSION:2.0',
    'PRODID:-//fvoci//fvoci//EN',
    'CALSCALE:GREGORIAN',
    `X-WR-CALNAME:${icsText(name)}`,
  ];
  const stamp = momentOf(now);
  for (const event of events) {
    const when = whenLines(event.when);
    if (!when) continue;
    lines.push(
      'BEGIN:VEVENT',
      `UID:${icsText(event.uid)}`,
      `DTSTAMP:${stamp}`,
      ...when,
      `SUMMARY:${icsText(event.title)}`,
    );
    const url = icsUrl(event.url);
    if (url) lines.push(`URL:${url}`);
    lines.push('END:VEVENT');
  }
  lines.push('END:VCALENDAR');
  return `${lines.map(foldLine).join('\r\n')}\r\n`;
}
