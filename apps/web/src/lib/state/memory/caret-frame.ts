/**
 * The carets of a room as their frames carry them (`contracts/websocket.md`,
 * section 6): an awareness update of y-protocols, read and written here as
 * the server reads and writes it (`crates/server/src/collab/caret.rs`), for
 * the rooms of the backend in memory. Both are held to
 * `contracts/caret-vectors.json`.
 *
 * Of what a client sends only where its caret is is read: whose it is, the
 * number it is known under and which telling it is are the room's to say.
 * Nothing here knows Yjs: a place is JSON.
 */

/** The most bytes of an awareness update a client sends that is read. */
export const CARET_BYTES = 1024;

const MOST_NUMBER = Number.MAX_SAFE_INTEGER;
/**
 * The one root type a place may name: the field of a body's Y document
 * (`editor/body-editor.ts`, `BODY_FIELD`). Another, named, would be made in
 * the document of whoever is told the caret.
 */
export const CARET_FIELD = 'default';

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

const isCount = (value: unknown): value is number =>
  typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= MOST_NUMBER;

/** An item's identifier as the server writes it; undefined when it is none. */
function idOf(value: unknown): Json | null | undefined {
  if (value === null || value === undefined) return null;
  if (!isObject(value) || Object.keys(value).some((key) => key !== 'client' && key !== 'clock')) {
    return undefined;
  }
  const { client, clock } = value;
  return isCount(client) && isCount(clock) ? { client, clock } : undefined;
}

/** A place as the server writes it: what it has, in its order, without what is null. */
function placeOf(value: unknown): Json | undefined {
  if (!isObject(value)) return undefined;
  const known = ['type', 'tname', 'item', 'assoc'];
  if (Object.keys(value).some((key) => !known.includes(key))) return undefined;
  const type = idOf(value.type);
  const item = idOf(value.item);
  const { tname, assoc } = value;
  if (type === undefined || item === undefined) return undefined;
  const place: Json = {};
  if (type) place.type = type;
  if (tname !== null && tname !== undefined) {
    if (tname !== CARET_FIELD) return undefined;
    place.tname = tname;
  }
  if (item) place.item = item;
  if (assoc !== null && assoc !== undefined) {
    if (typeof assoc !== 'number' || !Number.isInteger(assoc) || assoc < -128 || assoc > 127) {
      return undefined;
    }
    place.assoc = assoc;
  }
  // A place that names nothing is none: the editor's library cannot find it.
  if (!type && !item && place.tname === undefined) return undefined;
  return place;
}

/** A varuint of lib0 at `at`; undefined for one that does not end or is too large to count with. */
function readVaruint(bytes: Uint8Array, at: number): { value: number; next: number } | undefined {
  let value = 0;
  let scale = 1;
  for (let next = at; ; next += 1) {
    const byte = bytes[next];
    if (byte === undefined) return undefined;
    value += (byte % 128) * scale;
    if (byte < 128) return Number.isSafeInteger(value) ? { value, next: next + 1 } : undefined;
    scale *= 128;
    if (scale > Number.MAX_SAFE_INTEGER) return undefined;
  }
}

function writeVaruint(out: number[], value: number): void {
  let rest = value;
  while (rest > 127) {
    out.push((rest % 128) + 128);
    rest = Math.floor(rest / 128);
  }
  out.push(rest);
}

/**
 * What a client said of its caret in the awareness update it sent: the two
 * places as the server writes them, `null` for a state that shows none, and
 * undefined for what is no update of exactly one state, or no caret.
 */
export function caretSaid(update: Uint8Array): { anchor: Json; head: Json } | null | undefined {
  const count = readVaruint(update, 0);
  if (count?.value !== 1) return undefined;
  // Whose it says it is, and which telling: the room's to say, not read.
  const client = readVaruint(update, count.next);
  const clock = client && readVaruint(update, client.next);
  const length = clock && readVaruint(update, clock.next);
  if (length === undefined) return undefined;
  if (update.length - length.next !== length.value) return undefined;
  let state: unknown;
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(update.subarray(length.next));
    state = JSON.parse(text);
  } catch {
    return undefined;
  }
  if (state === null) return null;
  if (!isObject(state)) return undefined;
  const { cursor } = state;
  if (cursor === null || cursor === undefined) return null;
  if (!isObject(cursor) || Object.keys(cursor).some((key) => key !== 'anchor' && key !== 'head')) {
    return undefined;
  }
  const anchor = placeOf(cursor.anchor);
  const head = placeOf(cursor.head);
  return anchor && head ? { anchor, head } : undefined;
}

/** The awareness state the others are told for a caret of `userId`. */
export function caretState(userId: string, cursor: { anchor: Json; head: Json }): string {
  return JSON.stringify({ user: { id: userId }, cursor });
}

/** The awareness update that tells a caret under the room's number and clock; `null`: it is gone. */
export function caretTold(number: number, clock: number, state: string | null): Uint8Array {
  const text = new TextEncoder().encode(state ?? 'null');
  const head: number[] = [];
  writeVaruint(head, 1);
  writeVaruint(head, number);
  writeVaruint(head, clock);
  writeVaruint(head, text.length);
  const bytes = new Uint8Array(head.length + text.length);
  bytes.set(head);
  bytes.set(text, head.length);
  return bytes;
}
