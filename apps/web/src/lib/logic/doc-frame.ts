/**
 * The binary frame of a room (`contracts/websocket.md`, section 6), the same
 * in both directions: `doc`, kind, `epoch`, then a sync message of
 * y-protocols (its kind, a length, that many bytes). Every number is a
 * varuint of lib0: seven bits a byte from the low end, the top bit set on
 * every byte but the last.
 *
 * Nothing here knows Yjs: a payload is bytes.
 */

import type { RoomCarets, RoomFrame as DocFrame } from './doc-room-types.ts';

export type { DocFrame };

/** The kinds of sync message. */
export const SYNC_STEP_1 = 0;
export const SYNC_STEP_2 = 1;
export const SYNC_UPDATE = 2;

/** The kinds of frame: a sync message, and carets (an awareness update). */
const KIND_SYNC = 0;
const KIND_CARETS = 1;

function writeVaruint(out: number[], value: number): void {
  let rest = value;
  while (rest > 127) {
    out.push((rest % 128) + 128);
    rest = Math.floor(rest / 128);
  }
  out.push(rest);
}

export function encodeDocFrame(frame: DocFrame): Uint8Array<ArrayBuffer> {
  const head: number[] = [];
  writeVaruint(head, frame.doc);
  writeVaruint(head, KIND_SYNC);
  writeVaruint(head, frame.epoch);
  writeVaruint(head, frame.sync);
  writeVaruint(head, frame.payload.length);
  const bytes = new Uint8Array(head.length + frame.payload.length);
  bytes.set(head);
  bytes.set(frame.payload, head.length);
  return bytes;
}

/** A frame of carets: the room, the epoch, and the awareness update as it is. */
export function encodeCaretFrame(carets: RoomCarets): Uint8Array<ArrayBuffer> {
  const head: number[] = [];
  writeVaruint(head, carets.doc);
  writeVaruint(head, KIND_CARETS);
  writeVaruint(head, carets.epoch);
  writeVaruint(head, carets.update.length);
  const bytes = new Uint8Array(head.length + carets.update.length);
  bytes.set(head);
  bytes.set(carets.update, head.length);
  return bytes;
}

/** Reads varuints from the front of `bytes`: each call the next, undefined for one that does not end or is too large to count with. */
function reader(bytes: Uint8Array): { read: () => number | undefined; rest: () => Uint8Array } {
  let at = 0;
  const read = (): number | undefined => {
    let value = 0;
    let scale = 1;
    for (;;) {
      const byte = bytes[at];
      if (byte === undefined) return undefined;
      at += 1;
      value += (byte % 128) * scale;
      if (byte < 128) return Number.isSafeInteger(value) ? value : undefined;
      scale *= 128;
      if (scale > Number.MAX_SAFE_INTEGER) return undefined;
    }
  };
  return { read, rest: () => bytes.subarray(at) };
}

/**
 * Reads a frame of a sync message. Undefined for what is to be dropped (or
 * is a frame of carets): a frame of another kind, a sync message of no
 * known kind, a number that does not end or is too large to count with, a
 * length that is not the bytes that are left.
 */
export function decodeDocFrame(bytes: Uint8Array): DocFrame | undefined {
  const { read, rest } = reader(bytes);
  const doc = read();
  const kind = read();
  const epoch = read();
  const sync = read();
  const length = read();
  if (doc === undefined || epoch === undefined || sync === undefined || length === undefined) {
    return undefined;
  }
  if (kind !== KIND_SYNC || sync > SYNC_UPDATE) return undefined;
  const payload = rest();
  return length === payload.length ? { doc, epoch, sync, payload } : undefined;
}

/** Reads a frame of carets; undefined for any other frame, and for one whose length is not the bytes that are left. */
export function decodeCaretFrame(bytes: Uint8Array): RoomCarets | undefined {
  const { read, rest } = reader(bytes);
  const doc = read();
  const kind = read();
  const epoch = read();
  const length = read();
  if (doc === undefined || epoch === undefined || length === undefined) return undefined;
  if (kind !== KIND_CARETS) return undefined;
  const update = rest();
  return length === update.length ? { doc, epoch, update } : undefined;
}

/**
 * Updates as as few payloads as hold them, in their order, none over
 * `limit` bytes: merged (`merge`, Yjs's) where they fit together. Undefined
 * when one update alone is over the limit: an update is not cut.
 */
export function packUpdates(
  updates: readonly Uint8Array[],
  limit: number,
  merge: (updates: Uint8Array[]) => Uint8Array,
): Uint8Array[] | undefined {
  if (updates.length === 0) return [];
  const one = updates.length === 1 ? updates[0] : undefined;
  const merged = one ?? merge([...updates]);
  if (merged.length <= limit) return [merged];
  if (one) return undefined;
  const payloads: Uint8Array[] = [];
  let group: Uint8Array[] = [];
  let size = 0;
  const close = (): void => {
    const [single] = group;
    if (group.length === 1 && single) payloads.push(single);
    else if (group.length > 1) {
      const together = merge(group);
      // Merging makes updates smaller; were it ever not so, they go one by one.
      if (together.length <= limit) payloads.push(together);
      else payloads.push(...group);
    }
    group = [];
    size = 0;
  };
  for (const update of updates) {
    if (update.length > limit) return undefined;
    if (size + update.length > limit) close();
    group.push(update);
    size += update.length;
  }
  close();
  return payloads;
}

/**
 * The updates before the first that alone is over `limit`, as payloads
 * (`packUpdates`), and whether there was such a one.
 */
export function packFitting(
  updates: readonly Uint8Array[],
  limit: number,
  merge: (updates: Uint8Array[]) => Uint8Array,
): { payloads: Uint8Array[]; tooLarge: boolean } {
  const over = updates.findIndex((update) => update.length > limit);
  const fits = over === -1 ? updates : updates.slice(0, over);
  return { payloads: packUpdates(fits, limit, merge) ?? [], tooLarge: over !== -1 };
}
