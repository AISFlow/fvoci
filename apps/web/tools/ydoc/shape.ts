/**
 * The shape of a Y document's content, read and written by hand.
 *
 * `shapeOf` writes out what a shared type holds: which kind of type each node
 * is, the value and the JavaScript type of each attribute, the runs of each
 * text with their formatting. `rawType` goes the other way and makes shared
 * types from such a description without any editor's schema in between, so
 * that a document no editor would write can be made: a list without entries,
 * an element of a kind nobody knows. The tool that writes
 * `contracts/ydoc-vectors.json` makes its cases with these, and its test
 * reads the documents back with them.
 */
import * as Y from 'yjs';

/** What an attribute of an element may hold. */
export type AttributeValue = object | string | number | boolean | null;

/**
 * An element whose attributes hold values of any kind. The library's own type
 * says an attribute is a string, which is not what it stores.
 */
export type RawElement = Y.XmlElement<Record<string, AttributeValue>>;

/** A run of a text: its letters and the formatting they carry. */
export interface RawRun {
  insert: string;
  attributes?: Record<string, unknown>;
}

/** An element with its attributes and what is inside it, or a text. */
export type RawNode =
  | { element: string; attrs?: Record<string, AttributeValue>; children?: RawNode[] }
  | { text: RawRun[] };

/** The shared type a description stands for, not yet part of a document. */
export function rawType(node: RawNode): Y.XmlElement | Y.XmlText {
  if ('text' in node) {
    const text = new Y.XmlText();
    text.applyDelta(node.text);
    return text;
  }
  const element = new Y.XmlElement(node.element);
  for (const [name, value] of Object.entries(node.attrs ?? {}))
    (element as unknown as RawElement).setAttribute(name, value);
  element.insert(0, (node.children ?? []).map(rawType));
  return element;
}

/** An attribute as it is held: its value and what kind of value that is. */
export interface AttributeShape {
  type: string;
  value: unknown;
}

/** What a shared type holds. */
export type Shape =
  | { kind: 'XmlFragment'; children: Shape[] }
  | { kind: 'XmlElement'; name: string; attrs: Record<string, AttributeShape>; children: Shape[] }
  | { kind: 'XmlText'; runs: RawRun[] }
  | { kind: 'other'; what: string };

const kindOfValue = (value: unknown): string => {
  if (value === null) return 'null';
  return Array.isArray(value) ? 'array' : typeof value;
};

/** Writes out what a shared type holds, all the way down. */
export function shapeOf(type: unknown): Shape {
  // An element is a fragment too, and a text of a document is not: the order matters.
  if (type instanceof Y.XmlText) return { kind: 'XmlText', runs: type.toDelta() as RawRun[] };
  if (type instanceof Y.XmlElement) {
    const attrs = type.getAttributes() as Record<string, unknown>;
    return {
      kind: 'XmlElement',
      name: type.nodeName,
      attrs: Object.fromEntries(
        Object.entries(attrs).map(([name, value]) => [name, { type: kindOfValue(value), value }]),
      ),
      children: type.toArray().map(shapeOf),
    };
  }
  if (type instanceof Y.XmlFragment) {
    return { kind: 'XmlFragment', children: type.toArray().map(shapeOf) };
  }
  const named = typeof type === 'object' && type !== null ? type.constructor.name : typeof type;
  return { kind: 'other', what: named };
}

/** The shared type at a path of child indexes below a fragment. */
export function typeAt(root: Y.XmlFragment, path: readonly number[]): Y.XmlFragment | Y.XmlText {
  let current: Y.XmlFragment | Y.XmlText = root;
  for (const index of path) {
    if (current instanceof Y.XmlText) throw new Error('a text has nothing inside it');
    const next = current.get(index);
    if (!(next instanceof Y.XmlFragment) && !(next instanceof Y.XmlText))
      throw new Error(`nothing at index ${index}`);
    current = next;
  }
  return current;
}
