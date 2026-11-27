/**
 * The elements of an HWPX package's list (`content.hpf`) as rhwp (0.9.0)
 * sees them. Its reader is quick-xml 0.42 (`Reader::from_str`, the default
 * configuration), whose rules for where markup ends are its own, not XML's:
 * a quote opens a value anywhere in a tag, end tags hold quotes too, a
 * DOCTYPE holds them in places, and the attributes rhwp takes are the ones
 * quick-xml's iterator gives without an error. They are followed here as far
 * as they decide what is an element and what its attributes are, so that
 * what is counted is what rhwp reads (hwpx-package.ts).
 *
 * Where quick-xml stops with an error, rhwp refuses the file; so this throws
 * `UnreadableFile` there, and also past `TAG_ATTRIBUTE_LIMIT`.
 */
import { UnreadableFile } from './renderer.ts';

/**
 * The most attributes of one tag whose attributes are read: a Hancom list's
 * tags have three or four. Each name written counts, whether it is taken,
 * given twice, given no value or a value without quotes: quick-xml keeps a
 * name given a value without quotes and checks each name against those kept
 * before it, so a tag of many such names, a name repeated after them, costs
 * rhwp the square of their count (80,000 took it 29 s). A tag of a million
 * attributes took this reader 1.3 s and 180 MiB, and the list may be 30 MB.
 * Past it the file is refused, with the answer zip-guard gives an archive
 * past its limits.
 */
const TAG_ATTRIBUTE_LIMIT = 1000;

/** A start or empty tag: its name as written, and the attributes rhwp takes, in order. */
export interface Tag {
  name: string;
  attributes: () => [string, string][];
}

const isSpace = (char: string | undefined): boolean =>
  char === ' ' || char === '\r' || char === '\n' || char === '\t';

/** Where `mark` starts at or after `from`; throws when it does not come. */
function find(xml: string, mark: string, from: number): number {
  const at = xml.indexOf(mark, from);
  if (at < 0) throw new UnreadableFile();
  return at;
}

/** The `>` that ends a tag whose text starts at `from`, a quote holding until it closes. */
function tagEnd(xml: string, from: number): number {
  let quote: string | undefined;
  for (let at = from; at < xml.length; at += 1) {
    const char = xml[at];
    if (quote !== undefined) {
      if (char === quote) quote = undefined;
    } else if (char === '>') {
      return at;
    } else if (char === '"' || char === "'") {
      quote = char;
    }
  }
  throw new UnreadableFile();
}

/** The first of `marks` at or after `from`, or -1. */
function firstOf(xml: string, from: number, marks: string): number {
  for (let at = from; at < xml.length; at += 1) {
    if (marks.includes(xml.charAt(at))) return at;
  }
  return -1;
}

/** The declarations of an internal subset in which a quote holds until it closes. */
const QUOTED_DECLARATIONS = ['!ENTITY', '!ATTLIST', '!NOTATION'];

/** Where a DOCTYPE's internal subset, from `from` (after `[`), ends: at its `]`. */
function subsetEnd(xml: string, from: number): number {
  let at = from;
  for (;;) {
    const mark = firstOf(xml, at, ']<');
    if (mark < 0) throw new UnreadableFile();
    if (xml[mark] === ']') return mark;
    const markup = mark + 1;
    if (xml[markup] === '?') {
      at = find(xml, '?>', markup + 1) + 1;
    } else if (xml.startsWith('!--', markup)) {
      at = find(xml, '-->', markup + 3) + 3;
    } else if (xml.startsWith('!ELEMENT', markup)) {
      at = find(xml, '>', markup + 8) + 1;
    } else {
      const quoted = QUOTED_DECLARATIONS.find((name) => xml.startsWith(name, markup));
      at = quoted ? tagEnd(xml, markup + quoted.length) : find(xml, '>', markup);
      at += 1;
    }
  }
}

/** The `>` that ends a DOCTYPE whose text after `<!` starts at `from`. */
function doctypeEnd(xml: string, from: number): number {
  let at = from;
  for (;;) {
    const mark = firstOf(xml, at, `'"[>`);
    if (mark < 0) throw new UnreadableFile();
    const char = xml[mark] ?? '';
    if (char === '>') return mark;
    if (char === '[') return find(xml, '>', subsetEnd(xml, mark + 1) + 1);
    at = find(xml, char, mark + 1) + 1;
  }
}

/** The `>` that ends a comment, character data or a DOCTYPE at `open`. */
function bangEnd(xml: string, open: number): number {
  if (xml.startsWith('<!--', open)) return find(xml, '-->', open + 4) + 2;
  if (xml.startsWith('<![CDATA[', open)) return find(xml, ']]>', open + 9) + 2;
  // The keyword in any case of ASCII letters (no `u` flag: no other letter matches one).
  if (!/^<!doctype/i.test(xml.slice(open, open + 9))) throw new UnreadableFile();
  const end = doctypeEnd(xml, open + 2);
  // A DOCTYPE names its root.
  if (!/[^ \r\n\t]/.test(xml.slice(open + 9, end))) throw new UnreadableFile();
  return end;
}

/** The attributes quick-xml's iterator gives without an error (the ones `.flatten()` keeps). */
function attributesOf(content: string, from: number): [string, string][] {
  const found: [string, string][] = [];
  const keys = new Set<string>();
  const firstSpace = (at: number): number => {
    while (at < content.length && !isSpace(content[at])) at += 1;
    return at < content.length ? at : -1;
  };
  const skipSpace = (at: number): number => {
    while (at < content.length && isSpace(content[at])) at += 1;
    return at;
  };
  let offset = from;
  let names = 0;
  while (offset >= 0 && offset < content.length) {
    const start = skipSpace(offset);
    if (start >= content.length) break;
    names += 1;
    if (names > TAG_ATTRIBUTE_LIMIT) throw new UnreadableFile();
    let end = start + 1;
    while (end < content.length && content[end] !== '=' && !isSpace(content[end])) end += 1;
    if (end >= content.length) break;
    const equals = content[end] === '=' ? end : skipSpace(end + 1);
    if (equals >= content.length) break;
    if (content[equals] !== '=') {
      // A name with no value: an error, and the next name starts here.
      offset = equals;
      continue;
    }
    const key = content.slice(start, end);
    if (keys.has(key)) {
      // A name given twice: an error, and what follows is skipped to a blank.
      offset = firstSpace(equals);
      continue;
    }
    keys.add(key);
    const value = skipSpace(equals + 1);
    const quote = content[value];
    if (quote === undefined) break;
    if (quote !== '"' && quote !== "'") {
      offset = firstSpace(value);
      continue;
    }
    const close = content.indexOf(quote, value + 1);
    if (close < 0) break;
    found.push([key, content.slice(value + 1, close)]);
    offset = close + 1;
  }
  return found;
}

/** Throws where text holds an `&` with no `;` before the next `&` or its end. */
function checkReferences(text: string): void {
  if (!text.includes('&')) return;
  if (text.split('&').some((part, index) => index > 0 && !part.includes(';'))) {
    throw new UnreadableFile();
  }
}

/**
 * The names of the elements open, each kept as where it starts and how long
 * it is: a stack of numbers rather than of strings, as a list may open
 * millions of elements.
 */
class OpenNames {
  #slots = new Int32Array(64);
  #size = 0;

  push(start: number, length: number): void {
    if (this.#size + 2 > this.#slots.length) {
      const grown = new Int32Array(this.#slots.length * 2);
      grown.set(this.#slots);
      this.#slots = grown;
    }
    this.#slots[this.#size] = start;
    this.#slots[this.#size + 1] = length;
    this.#size += 2;
  }

  /** Closes the element last opened: whether it is named `name` in `xml` (false when none is). */
  close(xml: string, name: string): boolean {
    if (this.#size === 0) return false;
    this.#size -= 2;
    const start = this.#slots[this.#size] ?? 0;
    const length = this.#slots[this.#size + 1] ?? 0;
    return length === name.length && xml.startsWith(name, start);
  }
}

/**
 * Hands `visit` the start and empty tags of `xml`, in order, as rhwp's reader
 * gives them, each read no further than asked (its attributes only when
 * `attributes()` is called). Throws `UnreadableFile` where that reader stops
 * with an error: markup that does not end, an end tag that closes no element
 * of its name, an `&` that is no reference, markup it does not know; and when
 * a tag has more attributes than `TAG_ATTRIBUTE_LIMIT` (as they are read).
 */
export function readTags(xml: string, visit: (tag: Tag) => void): void {
  const open = new OpenNames();
  let at = 0;
  for (;;) {
    const start = xml.indexOf('<', at);
    checkReferences(xml.slice(at, start < 0 ? xml.length : start));
    if (start < 0) return;
    const next = xml[start + 1];
    let end: number;
    if (next === undefined) throw new UnreadableFile();
    if (next === '!') {
      end = bangEnd(xml, start);
    } else if (next === '?') {
      end = find(xml, '?>', start + 1) + 1;
      if (end - start < 3) throw new UnreadableFile();
    } else if (next === '/') {
      end = tagEnd(xml, start + 1);
      // Blanks after the name are let be; a name of blanks alone is kept as it is.
      let nameEnd = end;
      while (nameEnd > start + 2 && isSpace(xml[nameEnd - 1])) nameEnd -= 1;
      if (!open.close(xml, xml.slice(start + 2, nameEnd > start + 2 ? nameEnd : end))) {
        throw new UnreadableFile();
      }
    } else {
      end = tagEnd(xml, start + 1);
      const empty = end - 1 > start && xml[end - 1] === '/';
      const contentEnd = empty ? end - 1 : end;
      let nameEnd = start + 1;
      while (nameEnd < contentEnd && !isSpace(xml[nameEnd])) nameEnd += 1;
      if (!empty) open.push(start + 1, nameEnd - start - 1);
      const content = (): string => xml.slice(start + 1, contentEnd);
      visit({
        name: xml.slice(start + 1, nameEnd),
        attributes: () => attributesOf(content(), nameEnd - start - 1),
      });
    }
    at = end + 1;
  }
}
