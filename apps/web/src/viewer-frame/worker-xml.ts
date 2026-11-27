/**
 * The XML half of the DOM docx-preview runs on in the Word worker
 * (`docx-worker.ts`), where browsers have no `DOMParser`: a parser with
 * namespaces (saxes) that builds the few node kinds docx-preview reads, with
 * the few members it reads (its `XmlParser`: `childNodes.item`,
 * `attributes.item`, `localName`, `namespaceURI`, `nodeType`, `textContent`,
 * `firstElementChild`; elsewhere `firstChild.nodeName` and
 * `lookupNamespaceURI`).
 *
 * Every node made counts toward one ceiling for the whole file, so that
 * parsing cannot hold more than a long document needs; past it the parser
 * throws `TooMuch`. Every part is also checked, whatever its root is called,
 * for what docx-preview's reading of numberings would make of it (each list
 * times the levels of the list it uses, and the style sheet text it writes
 * for them) before docx-preview makes it.
 */
import { SaxesParser, type SaxesTagNS } from 'saxes';

/** The most XML nodes a document's parts may have together: several times a long report's. */
export const XML_NODE_LIMIT = 2_000_000;
/** The most list levels docx-preview's reading of a numbering part may make: many times a long report's. */
export const NUMBERING_LIMIT = 20_000;
/** The most style sheet text docx-preview may write for numberings, in characters: hundreds of times a long report's. */
export const NUMBERING_TEXT_LIMIT = 8_000_000;

/** Thrown when a document would make more than the worker takes on. */
export class TooMuch extends Error {}

/** A list docx-preview reads with `item(i)` and `length`, and `Array.from`. */
class ItemList<T> extends Array<T> {
  item(index: number): T | null {
    return this[index] ?? null;
  }
}

/** What an element without children or attributes has: one list for all of them. */
const NONE: ItemList<never> = Object.freeze(new ItemList<never>());

export class XmlAttr {
  readonly name: string;
  readonly localName: string;
  readonly namespaceURI: string | null;
  readonly value: string;

  constructor(name: string, localName: string, namespaceURI: string | null, value: string) {
    this.name = name;
    this.localName = localName;
    this.namespaceURI = namespaceURI;
    this.value = value;
  }
}

export class XmlText {
  readonly nodeType = 3;
  readonly nodeName = '#text';
  readonly data: string;

  constructor(data: string) {
    this.data = data;
  }

  get textContent(): string {
    return this.data;
  }
}

export class XmlElement {
  readonly nodeType = 1;
  readonly nodeName: string;
  readonly localName: string;
  readonly namespaceURI: string | null;
  readonly parent: XmlElement | undefined;
  childNodes: ItemList<XmlElement | XmlText> = NONE;
  attributes: ItemList<XmlAttr> = NONE;
  /** The namespaces this element declares, by prefix ('' for the default); none, mostly. */
  readonly #declared: Record<string, string> | undefined;

  constructor(
    nodeName: string,
    localName: string,
    namespaceURI: string | null,
    parent: XmlElement | undefined,
    declared: Record<string, string> | undefined,
  ) {
    this.nodeName = nodeName;
    this.localName = localName;
    this.namespaceURI = namespaceURI;
    this.parent = parent;
    this.#declared = declared;
  }

  add(child: XmlElement | XmlText): void {
    if (this.childNodes === NONE) this.childNodes = new ItemList();
    this.childNodes.push(child);
  }

  get firstChild(): XmlElement | XmlText | null {
    return this.childNodes[0] ?? null;
  }

  get firstElementChild(): XmlElement | null {
    return this.childNodes.find((node) => node instanceof XmlElement) ?? null;
  }

  get textContent(): string {
    const parts: string[] = [];
    const stack: (XmlElement | XmlText)[] = [this];
    while (stack.length > 0) {
      const node = stack.pop();
      if (node instanceof XmlText) parts.push(node.data);
      else if (node) for (const child of node.childNodes.toReversed()) stack.push(child);
    }
    return parts.join('');
  }

  lookupNamespaceURI(prefix: string | null): string | null {
    const uri = this.#declared?.[prefix ?? ''];
    if (uri !== undefined) return uri === '' ? null : uri;
    return this.parent?.lookupNamespaceURI(prefix) ?? null;
  }
}

export class XmlDocument {
  readonly nodeType = 9;
  readonly firstElementChild: XmlElement | null;

  constructor(root: XmlElement | null) {
    this.firstElementChild = root;
  }

  /** docx-preview asks for "parsererror"; this parser throws instead of making one. */
  getElementsByTagName(): XmlElement[] {
    return [];
  }
}

/** Counts the nodes made, for the whole file, against `XML_NODE_LIMIT`. */
export class NodeCount {
  #made = 0;

  add(count = 1): void {
    this.#made += count;
    if (this.#made > XML_NODE_LIMIT) throw new TooMuch('XML nodes');
  }
}

const childrenNamed = (element: XmlElement, name: string): XmlElement[] =>
  element.childNodes.filter(
    (node): node is XmlElement => node instanceof XmlElement && node.localName === name,
  );
const attribute = (element: XmlElement | undefined, name: string): string =>
  element?.attributes.find((each) => each.localName === name)?.value ?? '';

/** What the attributes of an element and all within it come to, as CSS might hold them: names, values. */
function attributeText(root: XmlElement): number {
  let length = 0;
  const stack: XmlElement[] = [root];
  while (stack.length > 0) {
    const element = stack.pop();
    if (!element) continue;
    for (const each of element.attributes) length += each.localName.length + each.value.length + 4;
    for (const child of element.childNodes) if (child instanceof XmlElement) stack.push(child);
  }
  return length;
}

/** A list's levels, summed: how many, their rules' text but for the numbering's id, and their "%n" (each grows by the id). */
interface ListText {
  levels: number;
  text: number;
  counters: number;
}

/**
 * What docx-preview's reading of numberings would make of a part with this
 * root, as it reads (by local names only): how many list levels, and about
 * how much style sheet text it writes for them, at most. Each level of each
 * numbering gets a few rules whose selectors carry the numbering's id, the
 * level's text with each "%n" made a `counter(…)` that names the id again,
 * and the level's properties and picture bullet spread in.
 */
export function numberingSize(root: XmlElement): { levels: number; text: number } {
  const bullet = Math.max(0, ...childrenNamed(root, 'numPicBullet').map(attributeText));
  const lists = new Map<string, ListText>();
  for (const list of childrenNamed(root, 'abstractNum')) {
    const id = attribute(list, 'abstractNumId');
    const sum = lists.get(id) ?? { levels: 0, text: 0, counters: 0 };
    for (const level of childrenNamed(list, 'lvl')) {
      const text = attribute(childrenNamed(level, 'lvlText')[0], 'val');
      sum.levels += 1;
      sum.text += 100 + text.length + 2 * attributeText(level) + bullet;
      sum.counters += (text.match(/%/g) ?? []).length;
    }
    lists.set(id, sum);
  }
  let levels = 0;
  let text = 0;
  for (const num of childrenNamed(root, 'num')) {
    const id = attribute(num, 'numId').length;
    const list = lists.get(attribute(childrenNamed(num, 'abstractNumId')[0], 'val'));
    if (!list) continue;
    levels += list.levels;
    // The selectors (five at most) and the counter names carry the id.
    text += list.text + list.levels * 5 * (id + 30) + list.counters * (id + 40);
  }
  return { levels, text };
}

function element(tag: SaxesTagNS, parent: XmlElement | undefined): XmlElement {
  const declared = Object.keys(tag.ns).length > 0 ? tag.ns : undefined;
  const made = new XmlElement(tag.name, tag.local, tag.uri || null, parent, declared);
  const attributes = Object.values(tag.attributes);
  if (attributes.length > 0) {
    made.attributes = new ItemList();
    for (const each of attributes) {
      made.attributes.push(new XmlAttr(each.name, each.local, each.uri || null, each.value));
    }
  }
  return made;
}

/** Parses XML text into the nodes above; throws on text that is not well formed. */
export function parseXml(text: string, count: NodeCount): XmlDocument {
  const parser = new SaxesParser({ xmlns: true, position: false });
  let root: XmlElement | null = null;
  const open: XmlElement[] = [];
  parser.on('opentag', (tag) => {
    const parent = open.at(-1);
    const made = element(tag, parent);
    count.add(1 + made.attributes.length);
    if (parent) parent.add(made);
    else root = made;
    open.push(made);
  });
  parser.on('closetag', () => {
    open.pop();
  });
  const addText = (data: string): void => {
    const parent = open.at(-1);
    if (!parent) return;
    count.add();
    parent.add(new XmlText(data));
  };
  parser.on('text', addText);
  parser.on('cdata', addText);
  parser.on('error', (error) => {
    throw error;
  });
  parser.write(text).close();
  const parsed = root as XmlElement | null;
  if (parsed) {
    const numbering = numberingSize(parsed);
    if (numbering.levels > NUMBERING_LIMIT || numbering.text > NUMBERING_TEXT_LIMIT) {
      throw new TooMuch('list levels');
    }
  }
  return new XmlDocument(parsed);
}

/** A `DOMParser` for docx-preview: XML only, every document counted against one ceiling. */
export function domParserFor(count: NodeCount): new () => {
  parseFromString(text: string, type: string): XmlDocument;
} {
  return class {
    parseFromString(text: string): XmlDocument {
      return parseXml(text, count);
    }
  };
}
