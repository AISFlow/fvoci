/**
 * A compound file (OLE: the container of HWP, and of the old Word and Excel
 * formats), read without trusting what it says: enough to tell an HWP
 * document from another compound file, and to know how large its streams
 * say they are before anything reads them.
 *
 * A compound file is a file system in a file. Its header names the sectors
 * of its allocation table (FAT), the FAT chains the sectors of each stream,
 * and a directory names the streams. Streams under 4096 bytes lie in 64-byte
 * sectors of their own (the mini stream, itself a stream of the root), chained
 * by a table of their own (the mini FAT). Every chain here is walked at most
 * as far as there are sectors, and anything out of place gives undefined:
 * the bytes are then not a kind the viewer draws.
 *
 * What the directory says a stream holds is all a reader reads of it (the
 * renderer's reader, Rust's `cfb`, stops there too), so the sum of what the
 * streams say is what reading them all can take. `verifyChains` also checks
 * that each stream has the sectors it says, and that no sector is in two
 * chains: the frame does that before its renderer gets the file.
 */

const SIGNATURE = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
/** The header's own bytes; it takes the room of a whole sector before sector 0. */
const HEADER_SIZE = 512;
/** Sector numbers from here up are markers (end of chain, free, a FAT's own). */
const LAST_SECTOR = 0xfffffff9;
const END_OF_CHAIN = 0xfffffffe;
const NO_ENTRY = 0xffffffff;
/** The header lists the first 109 sectors of the FAT itself. */
const HEADER_FAT_SECTORS = 109;
const ENTRY_SIZE = 128;
const MINI_SECTOR_SIZE = 64;
const MINI_STREAM_CUTOFF = 4096;
/** More directory entries than any document has; past this the file is not one the viewer reads. */
const ENTRY_LIMIT = 10_000;

const ROOT = 5;
const STORAGE = 1;
const STREAM = 2;

/** One entry of the directory, as it describes itself. */
export interface CompoundEntry {
  name: string;
  /** 1 a storage (a folder), 2 a stream, 5 the root; 0 an unused entry. */
  type: number;
  left: number;
  right: number;
  child: number;
  /** Its first sector: in the mini stream when it is under 4096 bytes, otherwise in the file. */
  start: number;
  /** What it says it holds, in bytes. */
  size: number;
}

export interface CompoundFile {
  bytes: Uint8Array;
  sectorSize: number;
  /** The allocation table: the sector after each sector of a chain. */
  fat: readonly number[];
  miniFat: readonly number[];
  /** The FAT's own sectors and the directory's: the chains that hold the file together. */
  fatSectors: readonly number[];
  difatSectors: readonly number[];
  directorySectors: readonly number[];
  miniFatSectors: readonly number[];
  entries: readonly CompoundEntry[];
}

const decoder = new TextDecoder('utf-16le');

/** Whether the bytes start as a compound file does, whatever follows. */
export function startsAsCompound(bytes: Uint8Array): boolean {
  return bytes.length >= HEADER_SIZE && SIGNATURE.every((byte, index) => bytes[index] === byte);
}

/** How many whole sectors the file has after its header. */
function sectorCount(bytes: Uint8Array, sectorSize: number): number {
  return Math.max(0, Math.floor((bytes.length - sectorSize) / sectorSize));
}

/** Where a sector starts in the file: after the header's sector. */
const sectorStart = (sector: number, size: number): number => (sector + 1) * size;

/**
 * The sectors of a chain from `start`, by `table`; undefined when it leaves
 * the table, comes back on itself or does not end.
 */
export function chain(table: readonly number[], start: number): number[] | undefined {
  const sectors: number[] = [];
  let at = start;
  while (at !== END_OF_CHAIN) {
    if (at > LAST_SECTOR || at >= table.length || sectors.length >= table.length) return undefined;
    sectors.push(at);
    at = table[at] ?? NO_ENTRY;
  }
  return sectors;
}

/** The bytes of sectors one after another, cut to `length`. */
function gather(
  bytes: Uint8Array,
  sectors: readonly number[],
  size: number,
  length: number,
): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(Math.min(length, sectors.length * size));
  return fill(out, sectors, size, (sector) => {
    const from = sectorStart(sector, size);
    return bytes.subarray(from, from + size);
  });
}

/** Fills `out` with the sectors' bytes one after another, as far as it takes them. */
function fill(
  out: Uint8Array<ArrayBuffer>,
  sectors: readonly number[],
  size: number,
  bytesOf: (sector: number) => Uint8Array,
): Uint8Array<ArrayBuffer> {
  const needed = Math.min(sectors.length, Math.ceil(out.length / size));
  for (let index = 0; index < needed; index += 1) {
    const at = index * size;
    out.set(bytesOf(sectors[index] ?? 0).subarray(0, out.length - at), at);
  }
  return out;
}

/** The numbers of a table held in `sectors`. */
function readTable(view: DataView, sectors: readonly number[], size: number): number[] {
  const table: number[] = [];
  for (const sector of sectors) {
    const from = sectorStart(sector, size);
    for (let at = 0; at < size; at += 4) table.push(view.getUint32(from + at, true));
  }
  return table;
}

/**
 * The sectors of the FAT (the header's first 109, then those the DIFAT
 * sectors list) and the DIFAT's own sectors.
 */
function fatSectorsOf(
  view: DataView,
  size: number,
  total: number,
): { fat: number[]; difat: number[] } | undefined {
  const count = view.getUint32(44, true);
  if (count > total) return undefined;
  const fat: number[] = [];
  for (let index = 0; index < Math.min(count, HEADER_FAT_SECTORS); index += 1) {
    fat.push(view.getUint32(76 + index * 4, true));
  }
  const difat: number[] = [];
  let next = view.getUint32(68, true);
  const perSector = size / 4 - 1;
  while (fat.length < count) {
    if (next >= total || difat.length >= total) return undefined;
    difat.push(next);
    const from = sectorStart(next, size);
    for (let index = 0; index < perSector && fat.length < count; index += 1) {
      fat.push(view.getUint32(from + index * 4, true));
    }
    next = view.getUint32(from + perSector * 4, true);
  }
  return fat.every((sector) => sector < total) ? { fat, difat } : undefined;
}

function readEntries(directory: Uint8Array, major: number): CompoundEntry[] | undefined {
  const view = new DataView(directory.buffer, directory.byteOffset, directory.byteLength);
  const entries: CompoundEntry[] = [];
  for (let at = 0; at + ENTRY_SIZE <= directory.length; at += ENTRY_SIZE) {
    const nameLength = Math.min(view.getUint16(at + 64, true), 64);
    const name = decoder.decode(directory.subarray(at, at + Math.max(0, nameLength - 2)));
    // A version 3 file keeps only the low half of a size; old writers left the high half unset.
    const high = view.getUint32(at + 124, true);
    if (major === 4 && high !== 0) return undefined;
    entries.push({
      name,
      type: directory[at + 66] ?? 0,
      left: view.getUint32(at + 68, true),
      right: view.getUint32(at + 72, true),
      child: view.getUint32(at + 76, true),
      start: view.getUint32(at + 116, true),
      size: view.getUint32(at + 120, true),
    });
  }
  return entries.length > 0 && entries.length <= ENTRY_LIMIT ? entries : undefined;
}

/** The file's structure, or undefined when the bytes are no compound file this reads. */
export function compoundFile(bytes: Uint8Array): CompoundFile | undefined {
  if (!startsAsCompound(bytes)) return undefined;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const major = view.getUint16(26, true);
  const shift = view.getUint16(30, true);
  // Version 3 has sectors of 512 bytes, version 4 of 4096; nothing else is a compound file.
  if (!((major === 3 && shift === 9) || (major === 4 && shift === 12))) return undefined;
  if (view.getUint16(28, true) !== 0xfffe || view.getUint16(32, true) !== 6) return undefined;
  if (view.getUint32(56, true) !== MINI_STREAM_CUTOFF) return undefined;
  const sectorSize = 1 << shift;
  const total = sectorCount(bytes, sectorSize);
  const tables = fatSectorsOf(view, sectorSize, total);
  if (!tables) return undefined;
  const { fat: fatSectors, difat: difatSectors } = tables;
  const fat = readTable(view, fatSectors, sectorSize).slice(0, total);
  const directorySectors = chain(fat, view.getUint32(48, true));
  // Counted before it is read: a directory past the limit is not read at all.
  if (!directorySectors || directorySectors.length * (sectorSize / ENTRY_SIZE) > ENTRY_LIMIT) {
    return undefined;
  }
  const directory = gather(bytes, directorySectors, sectorSize, Infinity);
  const entries = readEntries(directory, major);
  if (entries?.[0]?.type !== ROOT) return undefined;
  const miniFatStart = view.getUint32(60, true);
  const miniFatSectors = miniFatStart === END_OF_CHAIN ? [] : chain(fat, miniFatStart);
  if (!miniFatSectors) return undefined;
  const miniFat = readTable(view, miniFatSectors, sectorSize);
  return {
    bytes,
    sectorSize,
    fat,
    miniFat,
    fatSectors,
    difatSectors,
    directorySectors,
    miniFatSectors,
    entries,
  };
}

/** Whether an entry is a stream that lives in the mini stream. */
const isMini = (entry: CompoundEntry): boolean =>
  entry.type === STREAM && entry.size < MINI_STREAM_CUTOFF;

/** The children of a storage, by walking its tree of siblings (each entry once). */
export function childrenOf(file: CompoundFile, storage: CompoundEntry): CompoundEntry[] {
  const found: CompoundEntry[] = [];
  const seen = new Set<number>();
  const pending = [storage.child];
  while (pending.length > 0) {
    const id = pending.pop() ?? NO_ENTRY;
    const entry = file.entries[id];
    if (!entry || seen.has(id)) continue;
    seen.add(id);
    found.push(entry);
    pending.push(entry.left, entry.right);
  }
  return found;
}

/** The stream at the root named `name` (compared as the format does, ignoring case). */
export function rootStream(file: CompoundFile, name: string): CompoundEntry | undefined {
  const root = file.entries[0];
  if (!root) return undefined;
  const wanted = name.toUpperCase();
  return childrenOf(file, root).find(
    (entry) => entry.type === STREAM && entry.name.toUpperCase() === wanted,
  );
}

/** The sectors of a stream's chain: in the mini stream or in the file. */
function streamChain(file: CompoundFile, entry: CompoundEntry): number[] | undefined {
  if (entry.size === 0) return [];
  return chain(isMini(entry) ? file.miniFat : file.fat, entry.start);
}

/** Each file's mini stream once read; null when its root has no chain to read. */
const miniStreams = new WeakMap<CompoundFile, Uint8Array<ArrayBuffer> | null>();

/**
 * The mini stream (the root's own stream), read the first time a stream in
 * it is: a document of thousands of small streams reads it once, not once
 * for each.
 */
function miniStreamOf(file: CompoundFile): Uint8Array<ArrayBuffer> | undefined {
  let miniStream = miniStreams.get(file);
  if (miniStream === undefined) {
    const root = file.entries[0];
    const rootChain = root ? chain(file.fat, root.start) : undefined;
    miniStream =
      root && rootChain ? gather(file.bytes, rootChain, file.sectorSize, root.size) : null;
    miniStreams.set(file, miniStream);
  }
  return miniStream ?? undefined;
}

/**
 * The first `most` bytes of a stream (all of it when it is shorter); undefined
 * when its chain does not hold what it says.
 */
export function readStream(
  file: CompoundFile,
  entry: CompoundEntry,
  most: number,
): Uint8Array<ArrayBuffer> | undefined {
  const length = Math.min(entry.size, most);
  const sectors = streamChain(file, entry);
  if (!sectors) return undefined;
  if (!isMini(entry)) {
    if (sectors.length * file.sectorSize < entry.size) return undefined;
    return gather(file.bytes, sectors, file.sectorSize, length);
  }
  const miniStream = miniStreamOf(file);
  if (!miniStream || sectors.length * MINI_SECTOR_SIZE < entry.size) return undefined;
  return fill(new Uint8Array(length), sectors, MINI_SECTOR_SIZE, (sector) => {
    const from = sector * MINI_SECTOR_SIZE;
    return miniStream.subarray(from, from + MINI_SECTOR_SIZE);
  });
}

/** What the streams of the directory say they hold, together, in bytes. */
export function streamTotal(file: CompoundFile): number {
  let total = 0;
  for (const entry of file.entries) if (entry.type === STREAM) total += entry.size;
  return total;
}

/** Marks each sector in `used`; false when one was marked already. */
function claim(used: Uint8Array, sectors: readonly number[]): boolean {
  for (const sector of sectors) {
    if (sector >= used.length || used[sector] === 1) return false;
    used[sector] = 1;
  }
  return true;
}

/**
 * Whether every stream has the sectors its size takes, and no sector of the
 * file (or of the mini stream) is in two chains: then what a stream says it
 * holds is what it holds, and no part of the file is read as two streams.
 */
export function verifyChains(file: CompoundFile): boolean {
  const used = new Uint8Array(file.fat.length);
  const structure = [
    file.fatSectors,
    file.difatSectors,
    file.directorySectors,
    file.miniFatSectors,
  ];
  if (!structure.every((sectors) => claim(used, sectors))) return false;
  const root = file.entries[0];
  const rootChain = root ? chain(file.fat, root.start) : [];
  if (!root || !rootChain || rootChain.length * file.sectorSize < root.size) return false;
  if (!claim(used, rootChain)) return false;
  const miniUsed = new Uint8Array(Math.floor(root.size / MINI_SECTOR_SIZE));
  for (const entry of file.entries) {
    if (entry.type !== STREAM) continue;
    const sectors = streamChain(file, entry);
    const size = isMini(entry) ? MINI_SECTOR_SIZE : file.sectorSize;
    if (!sectors || sectors.length * size < entry.size) return false;
    if (!claim(isMini(entry) ? miniUsed : used, sectors)) return false;
  }
  return true;
}

export const COMPOUND_TYPES = { ROOT, STORAGE, STREAM } as const;
