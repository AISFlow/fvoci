/**
 * The strings of a workbook: one table for every text a cell shows, the
 * shared strings part first (a cell of type `s` names its entry), then the
 * texts cells hold themselves (inline strings, formulas' text results,
 * errors), each kept once however many cells show it.
 *
 * A shared string is the text of its runs; a run's formatting is not kept,
 * nor the phonetic guide (`rPh`) that Korean and Japanese files add beside
 * the text. Every string counts toward the workbook's ceilings on strings
 * and on their characters, and is cut at a cell's longest.
 */
import { readXml, STRING_LENGTH, unescapeText, type Ceiling } from './xlsx-xml.ts';

export class Strings {
  readonly table: string[] = [];
  readonly #index = new Map<string, number>();
  readonly #count: Ceiling;
  readonly #text: Ceiling;

  constructor(count: Ceiling, text: Ceiling) {
    this.#count = count;
    this.#text = text;
  }

  /** Adds a string as a new entry: the shared strings, which cells name by place. */
  push(value: string): number {
    const kept = value.length > STRING_LENGTH ? value.slice(0, STRING_LENGTH) : value;
    this.#count.add();
    this.#text.add(kept.length);
    this.table.push(kept);
    return this.table.length - 1;
  }

  /** The entry of a text a cell holds, added once. */
  intern(value: string): number {
    const kept = value.length > STRING_LENGTH ? value.slice(0, STRING_LENGTH) : value;
    const known = this.#index.get(kept);
    if (known !== undefined) return known;
    const at = this.push(kept);
    this.#index.set(kept, at);
    return at;
  }
}

/**
 * Collects the text of a string item (`si`, or a cell's `is`): its `t`
 * elements, not those of a phonetic guide. Hand it every event within the
 * item; `take` gives the text and starts over.
 */
export class ItemText {
  #parts: string[] = [];
  #length = 0;
  #inText = false;
  #phonetic = 0;

  open(name: string): void {
    if (name === 'rPh') this.#phonetic += 1;
    else if (name === 't' && this.#phonetic === 0) this.#inText = true;
  }

  close(name: string): void {
    if (name === 'rPh') this.#phonetic -= 1;
    else if (name === 't') this.#inText = false;
  }

  text(text: string): void {
    // Past a cell's longest the rest is not kept: it would be cut anyway.
    if (!this.#inText || this.#length > STRING_LENGTH) return;
    this.#parts.push(text);
    this.#length += text.length;
  }

  take(): string {
    const text = unescapeText(this.#parts.join(''));
    this.#parts = [];
    this.#length = 0;
    this.#inText = false;
    this.#phonetic = 0;
    return text;
  }
}

/** Reads the shared strings part into the table. */
export function readSharedStrings(text: string, strings: Strings): void {
  const item = new ItemText();
  let inItem = false;
  readXml(text, {
    open: (name) => {
      if (name === 'si') inItem = true;
      else if (inItem) item.open(name);
    },
    close: (name) => {
      if (name === 'si') {
        strings.push(item.take());
        inItem = false;
      } else if (inItem) item.close(name);
    },
    text: (value) => {
      if (inItem) item.text(value);
    },
  });
}
