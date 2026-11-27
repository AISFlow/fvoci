/**
 * The HTML half of the DOM docx-preview draws into in the Word worker
 * (`docx-worker.ts`), where browsers have no `document`: elements, text,
 * comments and fragments with the members docx-preview's renderer uses
 * (`createElement(NS)`, `appendChild`, `setAttribute`, `style`, `classList`,
 * and the properties `id`, `lang`, `href`, `src`, `colSpan`, `rowSpan`), and
 * `flatten`, which turns what was drawn into a flat list of steps the frame
 * builds the same tree from (`docx-build.ts`). Steps, not HTML: an HTML
 * parser would rebuild some of the tree otherwise (a picture's `div` inside
 * a paragraph closes the paragraph).
 *
 * Every node made counts toward a ceiling; past it every further one throws
 * `TooMuch`, and the ceiling stays passed, so that a renderer which catches
 * an error somewhere still ends with the document refused. The text in the
 * steps (names, values, text) is held to a length; past it `flatten` throws
 * `TooMuch` too.
 */
import { CLOSE, MEASURE, NAMESPACES, OPEN, TEXT, type Step } from './docx-protocol.ts';
import { TooMuch } from './worker-xml.ts';

/** The most nodes a document may draw: several times a long report's. */
export const HTML_NODE_LIMIT = 400_000;
/** The most text the steps may carry, in characters. */
export const STEPS_TEXT_LIMIT = 16_000_000;

const [HTML_NS, SVG_NS] = NAMESPACES;

/** Counts the nodes made against `HTML_NODE_LIMIT`; once past it, stays past. */
export class Budget {
  #made = 0;

  get passed(): boolean {
    return this.#made > HTML_NODE_LIMIT;
  }

  add(): void {
    this.#made += 1;
    if (this.passed) throw new TooMuch('elements');
  }
}

/** The base of every node: what `instanceof Node` and `Node.ELEMENT_NODE` find. */
export class Node {
  static readonly ELEMENT_NODE = 1;
  static readonly TEXT_NODE = 3;
  static readonly COMMENT_NODE = 8;
  static readonly DOCUMENT_FRAGMENT_NODE = 11;
  parentNode: Parent | null = null;
}

export class Text extends Node {
  readonly nodeType = 3;
  readonly nodeName = '#text';
  data: string;

  constructor(data: string) {
    super();
    this.data = data;
  }

  get textContent(): string {
    return this.data;
  }
}

export class Comment extends Node {
  readonly nodeType = 8;
  readonly nodeName = '#comment';
}

export class Parent extends Node {
  readonly childNodes: Node[] = [];

  appendChild<T extends Node>(child: T): T {
    if (child instanceof Fragment) {
      for (const each of child.childNodes.splice(0)) this.appendChild(each);
      return child;
    }
    child.parentNode?.removeChild(child);
    child.parentNode = this;
    this.childNodes.push(child);
    return child;
  }

  append(...children: (Node | string)[]): void {
    for (const child of children) {
      this.appendChild(typeof child === 'string' ? new Text(child) : child);
    }
  }

  removeChild<T extends Node>(child: T): T {
    const at = this.childNodes.indexOf(child);
    if (at >= 0) this.childNodes.splice(at, 1);
    child.parentNode = null;
    return child;
  }

  get firstChild(): Node | null {
    return this.childNodes[0] ?? null;
  }

  get firstElementChild(): Element | null {
    return this.childNodes.find((node) => node instanceof Element) ?? null;
  }

  get textContent(): string {
    return this.childNodes
      .map((node) => (node instanceof Text || node instanceof Parent ? node.textContent : ''))
      .join('');
  }

  set textContent(text: string) {
    for (const child of this.childNodes.splice(0)) child.parentNode = null;
    if (text !== '') this.appendChild(new Text(text));
  }

  /** Only what the renderer sets: nothing, or a no-break space. */
  set innerHTML(html: string) {
    this.textContent = html === '&nbsp;' ? ' ' : html;
  }
}

export class Fragment extends Parent {
  readonly nodeType = 11;
  readonly nodeName = '#document-fragment';
}

/** A style declaration: whatever is set on it, by camel-case or by CSS name. */
export type Style = Record<string, string | undefined>;

const cssName = (name: string): string =>
  name.startsWith('--') || name.includes('-')
    ? name
    : name.replace(/[A-Z]/g, (upper) => `-${upper.toLowerCase()}`);

/** The properties docx-preview sets that stand for attributes, and their attributes. */
const REFLECTED = {
  className: 'class',
  id: 'id',
  lang: 'lang',
  href: 'href',
  src: 'src',
  colSpan: 'colspan',
  rowSpan: 'rowspan',
} as const;

export class Element extends Parent {
  readonly nodeType = 1;
  readonly tagName: string;
  readonly namespaceURI: string;
  // Made when first used: most elements have neither.
  #attributes: Map<string, string> | undefined;
  #style: Style | undefined;

  constructor(tagName: string, namespaceURI: string) {
    super();
    this.tagName = tagName;
    this.namespaceURI = namespaceURI;
  }

  get nodeName(): string {
    return this.namespaceURI === HTML_NS ? this.tagName.toUpperCase() : this.tagName;
  }

  get attributes(): ReadonlyMap<string, string> {
    return this.#attributes ?? new Map();
  }

  get style(): Style {
    this.#style ??= {};
    return this.#style;
  }

  /** The style as a declaration list, or '' when nothing is set. */
  get styleText(): string {
    return Object.entries(this.#style ?? {})
      .filter((entry): entry is [string, string] => typeof entry[1] === 'string' && entry[1] !== '')
      .map(([name, value]) => `${cssName(name)}: ${value}`)
      .join('; ');
  }

  get classList(): { add: (...names: string[]) => void } {
    return {
      add: (...names) => {
        const now = (this.getAttribute('class') ?? '').split(' ').filter(Boolean);
        this.setAttribute('class', [...new Set([...now, ...names])].join(' '));
      },
    };
  }

  setAttribute(name: string, value: unknown): void {
    this.#attributes ??= new Map();
    this.#attributes.set(name, String(value));
  }

  getAttribute(name: string): string | null {
    return this.#attributes?.get(name) ?? null;
  }

  get className(): string {
    return this.getAttribute(REFLECTED.className) ?? '';
  }
  set className(value: string) {
    this.setAttribute(REFLECTED.className, value);
  }
  get id(): string {
    return this.getAttribute(REFLECTED.id) ?? '';
  }
  set id(value: string) {
    this.setAttribute(REFLECTED.id, value);
  }
  get lang(): string {
    return this.getAttribute(REFLECTED.lang) ?? '';
  }
  set lang(value: string) {
    this.setAttribute(REFLECTED.lang, value);
  }
  get href(): string {
    return this.getAttribute(REFLECTED.href) ?? '';
  }
  set href(value: string) {
    this.setAttribute(REFLECTED.href, value);
  }
  get src(): string {
    return this.getAttribute(REFLECTED.src) ?? '';
  }
  set src(value: string) {
    this.setAttribute(REFLECTED.src, value);
  }
  get colSpan(): number {
    return Number(this.getAttribute(REFLECTED.colSpan) ?? 1);
  }
  set colSpan(value: number) {
    this.setAttribute(REFLECTED.colSpan, value);
  }
  get rowSpan(): number {
    return Number(this.getAttribute(REFLECTED.rowSpan) ?? 1);
  }
  set rowSpan(value: number) {
    this.setAttribute(REFLECTED.rowSpan, value);
  }
}

/** The `document` docx-preview draws with: every node it makes is counted. */
export function documentFor(budget: Budget): Record<string, unknown> {
  const counted = <T>(make: () => T): T => {
    budget.add();
    return make();
  };
  return {
    createElement: (tag: string) => counted(() => new Element(tag.toLowerCase(), HTML_NS)),
    createElementNS: (namespace: string | null, tag: string) =>
      counted(() => {
        const element = new Element(
          namespace === HTML_NS ? tag.toLowerCase() : tag,
          namespace ?? HTML_NS,
        );
        // docx-preview makes an `svg` only for a shape, and measures each on the next frame:
        // the frame does that for it (`docx-build.ts`).
        if (namespace === SVG_NS && tag === 'svg') element.setAttribute(MEASURE, '');
        return element;
      }),
    createTextNode: (text: string) => counted(() => new Text(text)),
    createComment: () => counted(() => new Comment()),
    createDocumentFragment: () => counted(() => new Fragment()),
    createRange: () => {
      throw new Error('no layout in the worker');
    },
  };
}

/** What is left of the text the steps may carry, shared by the calls of `flatten` for one document. */
export interface Room {
  left: number;
}

/** The steps that build the nodes, in order; throws `TooMuch` once their text would pass what is left in `room`. */
export function flatten(nodes: readonly Node[], room: Room = { left: STEPS_TEXT_LIMIT }): Step[] {
  const steps: Step[] = [];
  const text = (value: string): string => {
    room.left -= value.length;
    if (room.left < 0) throw new TooMuch('drawn text');
    return value;
  };
  // Nodes to write, and CLOSE after an element's children.
  const stack: (Node | typeof CLOSE)[] = nodes.toReversed();
  while (stack.length > 0) {
    const next = stack.pop();
    if (next === CLOSE) steps.push(CLOSE);
    else if (next instanceof Text) steps.push(TEXT, text(next.data));
    else if (next instanceof Element) {
      const attributes = new Map(next.attributes);
      const style = next.styleText;
      if (style)
        attributes.set('style', [attributes.get('style'), style].filter(Boolean).join('; '));
      const namespace = (NAMESPACES as readonly string[]).indexOf(next.namespaceURI);
      steps.push(OPEN, text(next.tagName), Math.max(0, namespace), attributes.size);
      for (const [name, value] of attributes) steps.push(text(name), text(value));
      stack.push(CLOSE);
      // One at a time: a spread of a long list would overflow the call stack.
      for (const child of next.childNodes.toReversed()) stack.push(child);
    } else if (next instanceof Parent) {
      for (const child of next.childNodes.toReversed()) stack.push(child);
    }
  }
  return steps;
}
