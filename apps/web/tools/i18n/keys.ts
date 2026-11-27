/**
 * The key sets of the message files, held against the base locale.
 *
 * A locale may lack keys: the compiled message falls back to the base locale.
 * It may not have a key the base locale lacks, because nothing can call such
 * a message. The locales named as complete may not lack any key either.
 */

/** The locales that have to have every key the base locale has. */
export const COMPLETE_LOCALES = ['ko'];

export interface KeyDifference {
  /** In the base locale, not in the other. */
  missing: string[];
  /** In the other, not in the base locale. */
  extra: string[];
}

export function compareKeys(base: Iterable<string>, other: Iterable<string>): KeyDifference {
  const baseKeys = new Set(base);
  const otherKeys = new Set(other);
  return {
    missing: [...baseKeys].filter((key) => !otherKeys.has(key)),
    extra: [...otherKeys].filter((key) => !baseKeys.has(key)),
  };
}

/**
 * The message keys of a parsed message file. A string or an array is a
 * message; an object nests messages, whose keys are joined with a dot, as
 * the message format plugin does. `$schema` is not a message.
 */
export function messageKeys(file: unknown, prefix = ''): string[] {
  if (typeof file !== 'object' || file === null || Array.isArray(file)) return [];
  return Object.entries(file).flatMap(([key, value]) => {
    if (prefix === '' && key === '$schema') return [];
    const name = prefix + key;
    const nests = typeof value === 'object' && value !== null && !Array.isArray(value);
    return nests ? messageKeys(value, `${name}.`) : [name];
  });
}

/** One line per reason the key sets are not acceptable; none when they are. */
export function keyErrors(
  differences: ReadonlyMap<string, KeyDifference>,
  completeLocales: readonly string[],
): string[] {
  const errors: string[] = [];
  for (const [locale, { missing, extra }] of differences) {
    if (extra.length > 0) {
      errors.push(`${locale}: ${extra.length} key(s) the base locale does not have`);
    }
    if (completeLocales.includes(locale) && missing.length > 0) {
      errors.push(`${locale}: ${missing.length} key(s) missing; this locale has to be complete`);
    }
  }
  for (const locale of completeLocales) {
    if (!differences.has(locale)) errors.push(`${locale}: has to be complete but is not a locale`);
  }
  return errors;
}
