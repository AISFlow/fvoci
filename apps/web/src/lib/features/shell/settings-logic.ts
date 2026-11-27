/** What the settings dialog lists: time zones and languages. */

/** The setting that follows the device. */
export const AUTO = 'auto';

/** The time zones this browser knows by name; none where it cannot say. */
export function supportedTimeZones(): string[] {
  try {
    return Intl.supportedValuesOf('timeZone');
  } catch {
    return [];
  }
}

/**
 * The zones to choose from: those the browser knows, with `UTC` (which some
 * browsers leave out of their list) and the zone that is chosen now (which
 * may be a name this browser does not list), each once, in the order of
 * their names.
 */
export function timeZoneChoices(supported: readonly string[], chosen: string): string[] {
  const zones = new Set(supported);
  zones.add('UTC');
  if (chosen !== AUTO && chosen !== '') zones.add(chosen);
  return [...zones].sort((first, second) => (first < second ? -1 : first > second ? 1 : 0));
}

/** A zone's name as a reader reads it: `America/New_York` is "America/New York". */
export function timeZoneLabel(zone: string): string {
  return zone.replaceAll('_', ' ');
}

/**
 * A language named in that language ("한국어", "Deutsch"), with a capital
 * where the language has them; the tag itself where the browser has no name.
 */
export function languageName(tag: string): string {
  try {
    const name = new Intl.DisplayNames([tag], { type: 'language' }).of(tag);
    if (!name || name === tag) return tag;
    const [first] = new Intl.Segmenter(tag, { granularity: 'grapheme' }).segment(name);
    const head = first?.segment ?? '';
    return head.toLocaleUpperCase(tag) + name.slice(head.length);
  } catch {
    return tag;
  }
}
