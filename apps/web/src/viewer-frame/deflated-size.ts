/**
 * How much a deflated stream unpacks to, counted by reading its codes
 * without unpacking it: what the compound guard (compound-guard.ts) holds
 * an HWP document's streams to before rhwp reads them.
 *
 * rhwp (0.9.0) unpacks a stream with flate2's readers, which decode as zlib
 * does (zlib-rs): as raw deflate, then as zlib. A reader stops at the end of
 * the last block and passes over whatever follows it, and takes a stream cut
 * short as far as it goes. Hancom's own streams have bytes after their last
 * block; the browser's `DecompressionStream` fails on those, and drops what
 * the slice it failed in unpacked to, so it cannot count what rhwp takes.
 *
 * What a stream unpacks to depends on its codes alone, not on the bytes they
 * stand for, so nothing is written here: a literal counts one byte and a copy
 * its length. The count is never less than what zlib unpacks. What zlib
 * refuses past the point it stops at (a copy from before the start, a table
 * of codes left incomplete, lengths that say more than there is room for) is
 * read on here; only what no reader can read past ends the count early: a
 * block of the reserved type, a code that stands for nothing, a table with
 * more codes than its lengths have room for, a symbol past the alphabet.
 */

/** Thrown when the stream ends, or goes on in a way no reader can follow; one, made once. */
class StreamEnd extends Error {}
const END = new StreamEnd('the deflated stream ends');

/** The longest code, in bits. */
const MAX_BITS = 15;
/** Codes up to this long are looked up at once; longer ones are read a bit at a time. */
const FAST_BITS = 9;
/** For each 9 bits, those bits in the other order. */
const REVERSED = Uint16Array.from({ length: 1 << FAST_BITS }, (_, bits) => {
  let reversed = 0;
  for (let bit = 0; bit < FAST_BITS; bit += 1)
    reversed |= ((bits >> bit) & 1) << (FAST_BITS - 1 - bit);
  return reversed;
});

/** The order in which a block's lengths of the code-length code are written. */
const LENGTH_CODE_ORDER = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
/** The length of a copy for symbols 257 to 285, and how many bits more each reads. */
const COPY_BASE = [
  3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
  163, 195, 227, 258,
];
const COPY_EXTRA = [
  0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/** How many bits more each of the 30 distance symbols reads. */
const DISTANCE_EXTRA = [
  0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

/**
 * A table of prefix codes, by the canonical rule of deflate, made again in
 * place for each block from the symbols that have a code: a stream of many
 * small blocks allocates nothing for each, and a run of symbols without a
 * code costs nothing. The lookup table has a place for each `fastBits`
 * bits (the longest code, up to `FAST_BITS`) and is made only once the code
 * has read as many symbols as it has places; before that, a symbol is read
 * by the canonical rule, which gives the same one. Making the table costs
 * about twice its places (no code of `fastBits` or fewer has more symbols),
 * so it costs a block no more than the symbols it read first: what a block
 * costs follows the bits it takes, whatever its shape.
 */
class Code {
  /** How many codes are this many bits long. */
  readonly counts = new Uint16Array(MAX_BITS + 1);
  /** The symbols, by the length of their code and then by their value. */
  readonly symbols: Uint16Array;
  /** For each `fastBits` bits (first bit lowest): the symbol and its length, `symbol << 4 | length`; 0 for none. */
  readonly fast = new Int32Array(1 << FAST_BITS);
  /** How many bits `fast` is looked up by: the longest code, up to `FAST_BITS`. */
  fastBits = 0;
  /** Whether `fast` is made for the code as it is now. */
  hasFast = false;
  /** How many symbols were read by the canonical rule since the code was made. */
  uses = 0;
  readonly #next = new Uint16Array(MAX_BITS + 2);
  /** What the code was made of (see `set`), kept for `makeFast`. */
  #lengths: Uint8Array = new Uint8Array(0);
  #base = 0;
  #used: Uint16Array = new Uint16Array(0);
  #from = 0;
  #to = 0;

  constructor(most: number) {
    this.symbols = new Uint16Array(most);
  }

  /**
   * Takes the code in which symbol `s` has a code `lengths[base + s]` bits
   * long: `used[from..to)` are `base` plus the symbols that have one, in
   * order. False when the lengths have more codes than room.
   */
  set(lengths: Uint8Array, base: number, used: Uint16Array, from: number, to: number): boolean {
    const counts = this.counts;
    counts.fill(0);
    for (let at = from; at < to; at += 1) {
      const length = lengths[used[at] ?? 0] ?? 0;
      counts[length] = (counts[length] ?? 0) + 1;
    }
    return this.place(lengths, base, used, from, to);
  }

  /** `set`, with `counts` already counted from those lengths. */
  place(lengths: Uint8Array, base: number, used: Uint16Array, from: number, to: number): boolean {
    const counts = this.counts;
    let left = 1;
    let longest = 0;
    for (let length = 1; length <= MAX_BITS; length += 1) {
      left = 2 * left - (counts[length] ?? 0);
      if (left < 0) return false;
      if (counts[length] !== 0) longest = length;
    }
    this.fastBits = Math.min(longest, FAST_BITS);
    const offsets = this.#next;
    offsets[1] = 0;
    for (let length = 1; length <= MAX_BITS; length += 1) {
      offsets[length + 1] = (offsets[length] ?? 0) + (counts[length] ?? 0);
    }
    for (let at = from; at < to; at += 1) {
      const position = used[at] ?? 0;
      const length = lengths[position] ?? 0;
      const slot = offsets[length] ?? 0;
      offsets[length] = slot + 1;
      this.symbols[slot] = position - base;
    }
    [this.#lengths, this.#base, this.#used, this.#from, this.#to] = [lengths, base, used, from, to];
    this.hasFast = false;
    this.uses = 0;
    return true;
  }

  /** Makes the lookup table of the codes up to `FAST_BITS` long. */
  makeFast(): void {
    const next = this.#next;
    let code = 0;
    for (let length = 1; length <= MAX_BITS; length += 1) {
      code = (code + (this.counts[length - 1] ?? 0)) << 1;
      next[length] = code;
    }
    const fast = this.fast;
    const places = 1 << this.fastBits;
    fast.fill(0, 0, places);
    for (let at = this.#from; at < this.#to; at += 1) {
      const position = this.#used[at] ?? 0;
      const length = this.#lengths[position] ?? 0;
      const value = next[length] ?? 0;
      next[length] = value + 1;
      if (length > FAST_BITS) continue;
      // The code is read first bit first: the table is indexed by its bits reversed.
      const reversed = (REVERSED[value] ?? 0) >> (FAST_BITS - length);
      const entry = ((position - this.#base) << 4) | length;
      for (let fill = reversed; fill < places; fill += 1 << length) fast[fill] = entry;
    }
    this.hasFast = true;
  }
}

/** Puts in `used` the symbols that have a code in `lengths`, in order; how many. */
function usedOf(lengths: Uint8Array, used: Uint16Array): number {
  let count = 0;
  for (let symbol = 0; symbol < lengths.length; symbol += 1) {
    if (lengths[symbol] !== 0) {
      used[count] = symbol;
      count += 1;
    }
  }
  return count;
}

/** The bits of the stream, first bit of each byte first. */
class Bits {
  readonly #data: Uint8Array;
  #at: number;
  /** The bits read from the stream and not yet taken, `#held` of them. */
  #buffer = 0;
  #held = 0;

  constructor(data: Uint8Array, from: number) {
    this.#data = data;
    this.#at = from;
  }

  /** The bit the stream is read at, from the first of the data. */
  get position(): number {
    return this.#at * 8 - this.#held;
  }

  /** Holds at least `count` bits (up to 24), or as many as are left. */
  #fill(count: number): void {
    while (this.#held < count && this.#at < this.#data.length) {
      this.#buffer |= (this.#data[this.#at] ?? 0) << this.#held;
      this.#at += 1;
      this.#held += 8;
    }
  }

  /** The next `count` bits (up to 16), as a number. */
  take(count: number): number {
    if (count === 0) return 0;
    this.#fill(count);
    if (this.#held < count) throw END;
    const value = this.#buffer & ((1 << count) - 1);
    this.#buffer >>>= count;
    this.#held -= count;
    return value;
  }

  /**
   * Passes over the rest of the byte: a stored block starts on the next. The
   * whole bytes held ahead (a lookup holds up to two) are given back.
   */
  align(): void {
    this.#at -= this.#held >> 3;
    this.#buffer = 0;
    this.#held = 0;
  }

  /** Passes over up to `count` bytes (after `align`); how many there were. */
  skip(count: number): number {
    const skipped = Math.min(count, this.#data.length - this.#at);
    this.#at += skipped;
    return skipped;
  }

  /** The next symbol of `code`. */
  decode(code: Code): number {
    if (!code.hasFast) {
      code.uses += 1;
      if (code.uses > 1 << code.fastBits) code.makeFast();
    }
    if (code.hasFast) {
      this.#fill(code.fastBits);
      const entry = code.fast[this.#buffer & ((1 << code.fastBits) - 1)] ?? 0;
      const length = entry & 15;
      if (entry !== 0 && length <= this.#held) {
        this.#buffer >>>= length;
        this.#held -= length;
        return entry >> 4;
      }
    }
    // Before the table, longer than it, or near the end: a bit at a time, by the canonical rule.
    this.#fill(MAX_BITS);
    const most = Math.min(MAX_BITS, this.#held);
    let value = 0;
    let first = 0;
    let index = 0;
    for (let bits = 1; bits <= most; bits += 1) {
      value |= (this.#buffer >>> (bits - 1)) & 1;
      const count = code.counts[bits] ?? 0;
      if (value - first < count) {
        this.#buffer >>>= bits;
        this.#held -= bits;
        return code.symbols[index + value - first] ?? 0;
      }
      index += count;
      first = (first + count) << 1;
      value <<= 1;
    }
    throw END;
  }
}

let fixedCodes: [Code, Code] | undefined;

/** The codes of a block of fixed codes, made once. */
function fixed(): [Code, Code] {
  if (!fixedCodes) {
    const made = (lengths: Uint8Array): Code => {
      const used = new Uint16Array(lengths.length);
      const code = new Code(lengths.length);
      if (!code.set(lengths, 0, used, 0, usedOf(lengths, used))) {
        throw new Error('the fixed codes are full');
      }
      code.makeFast();
      return code;
    };
    const literals = new Uint8Array(288);
    literals.fill(8, 0, 144).fill(9, 144, 256).fill(7, 256, 280).fill(8, 280, 288);
    fixedCodes = [made(literals), made(new Uint8Array(30).fill(5))];
  }
  return fixedCodes;
}

/** The most lengths a block of its own codes gives: 288 literal and length symbols, 32 distances. */
const MOST_LENGTHS = 288 + 32;

/** The codes of a stream's blocks of their own codes, and what they are made of: made again for each. */
class Dynamic {
  readonly #lengthCodeLengths = new Uint8Array(19);
  readonly #lengthCodeUsed = new Uint16Array(19);
  readonly #lengthCode = new Code(19);
  readonly #lengths = new Uint8Array(MOST_LENGTHS);
  /** The places in `#lengths` of the lengths that are not 0, in order. */
  readonly #used = new Uint16Array(MOST_LENGTHS);
  readonly #literals = new Code(288);
  readonly #distances = new Code(32);

  /** The codes a block of its own codes describes at its start. */
  read(bits: Bits): [Code, Code] {
    const literalCount = bits.take(5) + 257;
    const distanceCount = bits.take(5) + 1;
    const lengthCodeCount = bits.take(4) + 4;
    const lengthCodeLengths = this.#lengthCodeLengths;
    lengthCodeLengths.fill(0);
    for (let index = 0; index < lengthCodeCount; index += 1) {
      lengthCodeLengths[LENGTH_CODE_ORDER[index] ?? 0] = bits.take(3);
    }
    const lengthCode = this.#lengthCode;
    const lengthCodeUsed = this.#lengthCodeUsed;
    const lengthCodes = usedOf(lengthCodeLengths, lengthCodeUsed);
    if (!lengthCode.set(lengthCodeLengths, 0, lengthCodeUsed, 0, lengthCodes)) throw END;
    // Only the lengths of the symbols in `used` are read again, so what the block before left is let be.
    const lengths = this.#lengths;
    const used = this.#used;
    const literals = this.#literals;
    const distances = this.#distances;
    literals.counts.fill(0);
    distances.counts.fill(0);
    const total = literalCount + distanceCount;
    let count = 0;
    let literalCodes = 0;
    let before = 0;
    for (let index = 0; index < total;) {
      const symbol = bits.decode(lengthCode);
      if (symbol < 16) {
        if (symbol !== 0) {
          lengths[index] = symbol;
          used[count] = index;
          count += 1;
          if (index < literalCount) {
            literals.counts[symbol] = (literals.counts[symbol] ?? 0) + 1;
            literalCodes += 1;
          } else {
            distances.counts[symbol] = (distances.counts[symbol] ?? 0) + 1;
          }
        }
        index += 1;
        before = symbol;
        continue;
      }
      // A repeat: of the length before (zlib refuses one with none before), or of none.
      const value = symbol === 16 ? before : 0;
      const times =
        symbol === 16 ? 3 + bits.take(2) : symbol === 17 ? 3 + bits.take(3) : 11 + bits.take(7);
      // Past the lengths there are room for, zlib refuses it; here the rest is filled.
      const end = Math.min(total, index + times);
      if (value !== 0) {
        const ofLiterals = Math.max(0, Math.min(end, literalCount) - index);
        literals.counts[value] = (literals.counts[value] ?? 0) + ofLiterals;
        distances.counts[value] = (distances.counts[value] ?? 0) + (end - index - ofLiterals);
        literalCodes += ofLiterals;
        for (; index < end; index += 1) {
          lengths[index] = value;
          used[count] = index;
          count += 1;
        }
      }
      index = end;
      before = value;
    }
    if (!literals.place(lengths, 0, used, 0, literalCodes)) throw END;
    if (!distances.place(lengths, literalCount, used, literalCodes, count)) throw END;
    return [literals, distances];
  }
}

/** Counts a deflated stream a block at a time. */
class Counter {
  readonly #bits: Bits;
  #dynamic: Dynamic | undefined;
  /** What the blocks read so far unpack to. */
  size = 0;
  /** Whether there is no block more to read: the last was read, the stream ended, or the count passed its room. */
  done = false;

  constructor(data: Uint8Array, from: number) {
    this.#bits = new Bits(data, from);
  }

  /** The bit the next block starts at. */
  get position(): number {
    return this.#bits.position;
  }

  /** Reads the next block, and stops as soon as the count passes `room`. */
  block(room: number): void {
    const bits = this.#bits;
    try {
      const last = bits.take(1);
      const type = bits.take(2);
      if (type === 3) {
        this.done = true;
        return;
      }
      if (type === 0) {
        bits.align();
        const length = bits.take(16);
        // Its complement, which zlib checks; whatever it says, the block is read on.
        bits.take(16);
        const copied = bits.skip(length);
        this.size += copied;
        this.done = last === 1 || copied < length || this.size > room;
        return;
      }
      const [literals, distances] =
        type === 1 ? fixed() : (this.#dynamic ??= new Dynamic()).read(bits);
      for (;;) {
        const symbol = bits.decode(literals);
        if (symbol < 256) {
          this.size += 1;
        } else if (symbol === 256) {
          break;
        } else {
          const at = symbol - 257;
          if (at >= COPY_BASE.length) throw END;
          const length = (COPY_BASE[at] ?? 0) + bits.take(COPY_EXTRA[at] ?? 0);
          const distance = bits.decode(distances);
          if (distance >= DISTANCE_EXTRA.length) throw END;
          bits.take(DISTANCE_EXTRA[distance] ?? 0);
          this.size += length;
        }
        if (this.size > room) {
          this.done = true;
          return;
        }
      }
      this.done = last === 1;
    } catch (error) {
      if (error !== END) throw error;
      this.done = true;
    }
  }

  /** Reads the rest of the blocks, as far as past `room`; what they all unpack to. */
  rest(room: number): number {
    while (!this.done) this.block(room);
    return this.size;
  }
}

/**
 * What the deflated stream that starts at `from` unpacks to, in bytes, read
 * as far as its last block (or as far as it goes); counted as far as past
 * `room`.
 */
export function deflatedSize(data: Uint8Array, from: number, room: number): number {
  return new Counter(data, from).rest(room);
}

/**
 * The larger of what the streams that start at `first` and at `second`
 * unpack to (each as `deflatedSize` counts it), counted as far as past
 * `room`. The two are read a block at a time, the one behind first. What a
 * block unpacks to depends on its bits alone, so once both are to read a
 * block at the same bit, they read the same blocks from there on: the rest
 * is read once, by the one that has unpacked more.
 */
export function largerDeflatedSize(
  data: Uint8Array,
  first: number,
  second: number,
  room: number,
): number {
  const one = new Counter(data, first);
  const other = new Counter(data, second);
  while (!one.done && !other.done) {
    if (one.position === other.position) {
      return (one.size >= other.size ? one : other).rest(room);
    }
    (one.position < other.position ? one : other).block(room);
  }
  const [ended, going] = one.done ? [one, other] : [other, one];
  return ended.size > room ? ended.size : Math.max(ended.size, going.rest(room));
}
