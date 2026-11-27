/**
 * Deflate streams written block by block, for the tests of the counter
 * (src/viewer-frame/deflated-size.ts): blocks of their own codes shaped to
 * cost a counter the most for the bits they take.
 */

/** The order in which a block's lengths of the code-length code are written. */
const ORDER = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/** The extra bits of the repeats 16, 17 and 18 of the code-length code. */
const REPEAT_BITS: Readonly<Record<number, number>> = { 16: 2, 17: 3, 18: 7 };

/** The bits of a stream, first bit of each byte first; a code is written first bit highest. */
class BitWriter {
  readonly #bits: number[] = [];

  get length(): number {
    return this.#bits.length;
  }

  put(value: number, count: number): void {
    for (let bit = 0; bit < count; bit += 1) this.#bits.push((value >> bit) & 1);
  }

  code(value: number, length: number): void {
    for (let bit = length - 1; bit >= 0; bit -= 1) this.#bits.push((value >> bit) & 1);
  }

  bytes(): Buffer {
    const bytes = Buffer.alloc(Math.ceil(this.#bits.length / 8));
    this.#bits.forEach((bit, at) => {
      bytes[at >> 3] = (bytes[at >> 3] ?? 0) | (bit << (at & 7));
    });
    return bytes;
  }
}

/** The canonical code of each symbol for these lengths, by the rule of deflate (0 for none). */
function canonical(lengths: readonly number[]): number[] {
  const counts = Array.from({ length: 16 }, () => 0);
  for (const length of lengths) if (length !== 0) counts[length] = (counts[length] ?? 0) + 1;
  const next = Array.from({ length: 16 }, () => 0);
  let code = 0;
  for (let length = 1; length < 16; length += 1) {
    code = (code + (counts[length - 1] ?? 0)) << 1;
    next[length] = code;
  }
  return lengths.map((length) => {
    if (length === 0) return 0;
    const value = next[length] ?? 0;
    next[length] = value + 1;
    return value;
  });
}

/** A block of its own codes, as its header and body write it. */
export interface Shape {
  /** Literal and copy symbols past 257, and distances past 1, as the header writes them. */
  readonly literals: number;
  readonly distances: number;
  /** The lengths of the code-length code, by its symbol. */
  readonly lengthCode: Readonly<Record<number, number>>;
  /** The lengths of the literals and distances, as symbols of the code-length code and a repeat's extra bits. */
  readonly lengths: readonly (readonly [symbol: number, extra?: number])[];
  /** The body before its end: a literal, or a copy of 3 (symbol 257) from distance symbol `copy`. */
  readonly body: readonly ({ readonly literal: number } | { readonly copy: number })[];
  /** What the block unpacks to. */
  readonly unpacks: number;
}

const times = <T>(count: number, item: T): T[] => Array.from({ length: count }, () => item);

/** The shapes that cost the counter the most, each named for what it makes the counter do. */
export const SHAPES = {
  // Two literals of 1 bit (0 and the end) and a distance, in 91 bits: the end only.
  'two codes and the end': {
    literals: 0,
    distances: 0,
    lengthCode: { 18: 1, 1: 1 },
    lengths: [[1], [18, 127], [18, 106], [1], [1]],
    body: [],
    unpacks: 0,
  },
  // A literal and 17 copies: each of the three codes reads as many symbols as its table has places.
  'a table for each code': {
    literals: 1,
    distances: 0,
    lengthCode: { 0: 1, 18: 3, 1: 3, 2: 2 },
    lengths: [[2], ...times(12, [0] as const), [18, 127], [18, 94], [2], [1], [1]],
    body: [{ literal: 0 }, ...times(17, { copy: 0 })],
    unpacks: 52,
  },
  // 286 literals of 9 bits, written with repeats, and 17 of them read.
  '286 codes of 9 bits': {
    literals: 29,
    distances: 0,
    lengthCode: { 9: 1, 16: 1 },
    lengths: [[9], ...times(47, [16, 3] as const), [16, 0], [9]],
    body: times(17, { literal: 0 }),
    unpacks: 17,
  },
  // All 320 lengths (past the 286 and 30 symbols zlib takes) 9 bits long, written with repeats.
  '320 codes of 9 bits': {
    literals: 31,
    distances: 31,
    lengthCode: { 9: 1, 16: 1 },
    lengths: [[9], ...times(53, [16, 3] as const), [9]],
    body: [],
    unpacks: 0,
  },
  // All 316 lengths 15 bits long, each written alone (the code-length code has codes of 1 bit).
  '316 codes of 15 bits': {
    literals: 29,
    distances: 29,
    lengthCode: { 15: 1, 0: 1 },
    lengths: times(316, [15] as const),
    body: [],
    unpacks: 0,
  },
} as const satisfies Record<string, Shape>;

function writeBlock(writer: BitWriter, shape: Shape): void {
  writer.put(0, 1);
  writer.put(2, 2);
  let lengthCodes = ORDER.length;
  while (lengthCodes > 4 && !shape.lengthCode[ORDER[lengthCodes - 1] ?? 0]) lengthCodes -= 1;
  writer.put(shape.literals, 5);
  writer.put(shape.distances, 5);
  writer.put(lengthCodes - 4, 4);
  for (const symbol of ORDER.slice(0, lengthCodes)) writer.put(shape.lengthCode[symbol] ?? 0, 3);
  const lengthCodeLengths = Array.from(
    { length: 19 },
    (_, symbol) => shape.lengthCode[symbol] ?? 0,
  );
  const lengthCodeCodes = canonical(lengthCodeLengths);
  const lengths: number[] = [];
  for (const [symbol, extra = 0] of shape.lengths) {
    writer.code(lengthCodeCodes[symbol] ?? 0, lengthCodeLengths[symbol] ?? 0);
    writer.put(extra, REPEAT_BITS[symbol] ?? 0);
    if (symbol < 16) lengths.push(symbol);
    else if (symbol === 16) lengths.push(...times(3 + extra, lengths.at(-1) ?? 0));
    else lengths.push(...times((symbol === 17 ? 3 : 11) + extra, 0));
  }
  if (lengths.length !== 258 + shape.literals + shape.distances) {
    throw new Error(`the shape gives ${String(lengths.length)} lengths`);
  }
  const literalLengths = lengths.slice(0, 257 + shape.literals);
  const distanceLengths = lengths.slice(257 + shape.literals);
  const literals = canonical(literalLengths);
  const distances = canonical(distanceLengths);
  const literal = (symbol: number): void => {
    writer.code(literals[symbol] ?? 0, literalLengths[symbol] ?? 0);
  };
  for (const part of shape.body) {
    if ('literal' in part) {
      literal(part.literal);
    } else {
      literal(257);
      writer.code(distances[part.copy] ?? 0, distanceLengths[part.copy] ?? 0);
    }
  }
  literal(256);
}

/** About `bytes` of blocks of `shape` (eight at a time, which end on a whole byte), and what they unpack to. */
export function blocksOf(shape: Shape, bytes: number): { data: Buffer; unpacks: number } {
  const writer = new BitWriter();
  for (let block = 0; block < 8; block += 1) writeBlock(writer, shape);
  const eight = writer.bytes();
  const count = Math.max(1, Math.floor(bytes / eight.length));
  return {
    data: Buffer.concat(Array.from({ length: count }, () => eight)),
    unpacks: count * 8 * shape.unpacks,
  };
}

/**
 * Bytes before a stream that lead both starts the guard reads from (0, and
 * 2 past a zlib header) to one bit, each with what it unpacks to from each
 * start.
 */
export const BOTH_STARTS_HEADS = {
  // At 0, a stored block of 10 bytes; at 2, one of 8: both go on at byte 15.
  'the first ahead': {
    head: Buffer.from([0, 10, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    unpacks: [10, 8],
  },
  // At 0, stored blocks of none and of 5; at 2, one of 8: both go on at byte 15.
  'the second ahead': {
    head: Buffer.from([0, 0, 0, 8, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0]),
    unpacks: [5, 8],
  },
  // At 0, a block of fixed codes with two literals; at 2, one with its end only: both go on at bit 26.
  'inside a byte': { head: Buffer.from([0x8a, 0x71, 0x02, 0x00]), unpacks: [2, 0] },
} as const;
