/** An iCalendar file of dated things (`icsCalendar`). */
import { describe, expect, it } from 'vitest';

import { calendarDate } from '$lib/logic/dates.ts';
import { foldLine, icsCalendar, icsText, icsUrl } from '$lib/logic/ics.ts';

const NOW = Date.UTC(2026, 10, 28, 9, 30, 0);

describe('an iCalendar file', () => {
  it('writes days with the day after the last as their end, and moments in UTC', () => {
    const file = icsCalendar(
      'Plans',
      [
        {
          uid: 'a@fvoci',
          title: 'Three days',
          when: {
            from: { date: calendarDate(2026, 12, 3) },
            to: { date: calendarDate(2026, 12, 5) },
          },
        },
        { uid: 'b@fvoci', title: 'One day', when: { from: { date: calendarDate(2026, 12, 31) } } },
        {
          uid: 'c@fvoci',
          title: 'A meeting',
          when: {
            from: { instant: Date.UTC(2026, 11, 1, 1, 0, 0) },
            to: { instant: Date.UTC(2026, 11, 1, 2, 30, 0) },
          },
        },
        { uid: 'd@fvoci', title: 'No day', when: {} },
      ],
      NOW,
    );
    const lines = file.split('\r\n');
    expect(file.endsWith('END:VCALENDAR\r\n')).toBe(true);
    expect(lines.filter((line) => line === 'BEGIN:VEVENT')).toHaveLength(3);
    expect(lines).toContain('DTSTART;VALUE=DATE:20261203');
    expect(lines).toContain('DTEND;VALUE=DATE:20261206');
    expect(lines).toContain('DTEND;VALUE=DATE:20270101');
    expect(lines).toContain('DTSTART:20261201T010000Z');
    expect(lines).toContain('DTEND:20261201T023000Z');
    expect(lines.filter((line) => line === 'DTSTAMP:20261128T093000Z')).toHaveLength(3);
    expect(lines).toContain('UID:a@fvoci');
    expect(file).not.toContain('No day');
  });

  it('escapes what a value may not hold as it is', () => {
    expect(icsText('a, b; c\\d\ne')).toBe('a\\, b\\; c\\\\d\\ne');
  });

  it('holds no control character in a text, and no break of its own in a line', () => {
    expect(icsText('a\u0000b\u0007c\u001Bd\u007Fe\tf')).toBe('abcde\tf');
    expect(foldLine('SUMMARY:one\r\nURL:javascript:x')).toBe('SUMMARY:oneURL:javascript:x');
  });

  it('links an event to an address of the web alone', () => {
    expect(icsUrl('https://fvoci.dev/w/a/b')).toBe('https://fvoci.dev/w/a/b');
    expect(icsUrl('http://localhost:5173/x')).toBe('http://localhost:5173/x');
    expect(icsUrl('javascript:alert(1)')).toBeUndefined();
    expect(icsUrl('https://fvoci.dev/\r\nATTENDEE:mailto:x@y')).toBeUndefined();
    expect(icsUrl('not a link')).toBeUndefined();
    expect(icsUrl(undefined)).toBeUndefined();
    const file = icsCalendar(
      'Plans',
      [
        {
          uid: 'a@fvoci',
          title: 'Injected',
          when: { from: { date: calendarDate(2026, 12, 3) } },
          url: 'https://fvoci.dev/\r\nATTENDEE:mailto:x@y',
        },
      ],
      NOW,
    );
    expect(file).not.toContain('URL:');
    expect(file).not.toContain('ATTENDEE');
  });

  it('reads a range that ends before it starts the other way round, and one moment as no range', () => {
    const lines = icsCalendar(
      'Plans',
      [
        {
          uid: 'a@fvoci',
          title: 'Backwards days',
          when: {
            from: { date: calendarDate(2026, 12, 5) },
            to: { date: calendarDate(2026, 12, 3) },
          },
        },
        {
          uid: 'b@fvoci',
          title: 'Backwards moments',
          when: {
            from: { instant: Date.UTC(2026, 11, 1, 2, 30, 0) },
            to: { instant: Date.UTC(2026, 11, 1, 1, 0, 0) },
          },
        },
        {
          uid: 'c@fvoci',
          title: 'A moment',
          when: {
            from: { instant: Date.UTC(2026, 11, 2, 9, 0, 0) },
            to: { instant: Date.UTC(2026, 11, 2, 9, 0, 0) },
          },
        },
      ],
      NOW,
    ).split('\r\n');
    expect(lines).toContain('DTSTART;VALUE=DATE:20261203');
    expect(lines).toContain('DTEND;VALUE=DATE:20261206');
    expect(lines).toContain('DTSTART:20261201T010000Z');
    expect(lines).toContain('DTEND:20261201T023000Z');
    expect(lines).toContain('DTSTART:20261202T090000Z');
    expect(lines).not.toContain('DTEND:20261202T090000Z');
  });

  it('folds a long line at 75 octets, never inside a character', () => {
    const line = `SUMMARY:${'가'.repeat(40)}`;
    const folded = foldLine(line);
    const parts = folded.split('\r\n');
    const encoder = new TextEncoder();
    expect(parts.length).toBeGreaterThan(1);
    expect(encoder.encode(parts[0]).length).toBeLessThanOrEqual(75);
    for (const part of parts.slice(1)) {
      expect(part.startsWith(' ')).toBe(true);
      expect(encoder.encode(part).length).toBeLessThanOrEqual(75);
    }
    expect(parts.map((part, at) => (at === 0 ? part : part.slice(1))).join('')).toBe(line);
    expect(foldLine('SHORT')).toBe('SHORT');
  });
});
