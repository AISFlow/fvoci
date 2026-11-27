/**
 * Whether what a person typed can be saved as the value of a property. The
 * value is saved as typed; these only decide whether it is saved at all. An
 * empty text is always fine: it clears the value.
 */

/**
 * A number property takes a text that starts with a whole number (`12`,
 * `-3`, `12.5`, and also `12 kg`); `abc` and `.5` are refused. The calculations read the stored text the same lenient way.
 */
export function isNumberEntry(text: string): boolean {
  return text === '' || !Number.isNaN(Number.parseInt(text, 10));
}

const EMAIL = /^[^\s@<>()[\]\\,;:"]+@(?:[\p{L}\p{N}-]+\.)+\p{L}{2,}$/u;

/** An email property takes one address: a name, an at sign, a host with a dot in it. */
export function isEmailEntry(text: string): boolean {
  return text === '' || EMAIL.test(text);
}

/**
 * A URL property takes anything that could be an address: no white space
 * inside, and at least one letter or digit. Whether it is shown as a link is
 * decided when it is shown, by its scheme.
 */
export function isUrlEntry(text: string): boolean {
  const address = text.trim();
  return text === '' || (!/\s/.test(address) && /[\p{L}\p{N}]/u.test(address));
}
